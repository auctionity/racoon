use rayon::prelude::*;
use rug::{float::Special, integer::Order, ops::Pow, Float, Integer};
use sha3::{Digest, Sha3_256};

/// Result of a simulation.
pub struct Result {
    /// Amount of simulated rounds.
    pub rounds: u64,
    /// Number of wins for each validator.
    pub wins: Vec<u64>,
    /// Lowest weight winning a block.
    pub min_win_weight: Float,
    /// Highest weight winning a block.
    pub max_win_weight: Float,
    /// Sum of all weights (to compute average)
    pub sum_win_weight: Float,
    /// Maximum of shards on which the same validator won at the same height.
    pub max_win_across_shards: u64,
}

impl Result {
    /// Create a default result with given validators amount and float precision.
    pub fn new(validators: usize, precision: u32) -> Self {
        Self {
            rounds: 0,
            wins: vec![0; validators],
            min_win_weight: Float::with_val(precision, Special::Infinity),
            max_win_weight: Float::with_val(precision, Special::NegInfinity),
            sum_win_weight: Float::with_val(precision, 0),
            max_win_across_shards: 0,
        }
    }

    /// Merge 2 results together.
    pub fn merge(mut a: Self, b: Self) -> Self {
        a.rounds += b.rounds;
        a.min_win_weight.min_mut(&b.min_win_weight);
        a.max_win_weight.max_mut(&b.max_win_weight);
        a.sum_win_weight += b.sum_win_weight;

        for (wa, wb) in a.wins.iter_mut().zip(b.wins.iter()) {
            *wa += wb;
        }

        if b.max_win_across_shards > a.max_win_across_shards {
            a.max_win_across_shards = b.max_win_across_shards;
        }

        a
    }

    /// Display the results in a human readable format.
    pub fn display(&self, powers: &[Float], top_amount: usize, precision: u32) {
        let win_rates: Vec<_> = self
            .wins
            .iter()
            .map(|w| Float::with_val(precision, w) / Float::with_val(precision, self.rounds))
            .collect();

        println!("Results (top {} validators) :", top_amount);
        println!("power         win rate       wins    diff");

        for i in 0..top_amount {
            let diff = Float::with_val(precision, &win_rates[i] - &powers[i]);
            println!(
                "{:0.8}    {:0.8} {:>8}    {:+0.8}",
                powers[i].to_f64(),
                win_rates[i].to_f64(),
                self.wins[i],
                diff.to_f64()
            );
        }

        println!();
        println!("blocks: {}", self.rounds);
        println!("max multi shard win : {}", self.max_win_across_shards);
        println!("min winner score : {:0.8}", self.min_win_weight.to_f64());
        println!("max winner score : {:0.8}", self.max_win_weight.to_f64());
        println!(
            "avr winner score : {:0.8}",
            Float::with_val(
                precision,
                &self.sum_win_weight / Float::with_val(precision, self.rounds)
            )
            .to_f64()
        );
    }
}

/// Configuration of the simulation.
pub struct Config<'a, W, P>
where
    // Weight forumla (seed, power, height, shard, validator, precision)
    W: Sync + Fn(&[u8], &Float, u64, u64, u64, u32) -> Float,
    P: Sync + Fn(),
{
    /// List of validators powers.
    /// They should sum up to 1.
    pub powers: &'a [Float],
    /// Weight formula.
    pub weight: &'a W,
    /// Callback triggered each time an epoch has been calculated for one shard.
    /// Mainly used for progress monitoring.
    pub progress: P,
    /// Amount of validators.
    pub validators: usize,
    /// Amount of shards.
    pub shards: u64,
    /// Amount of epochs.
    pub epochs: u64,
    /// Amount of blocks per epoch.
    pub blocks_per_epoch: u64,
    /// Float precision.
    pub precision: u32,
}

impl<'a, W, P> Config<'a, W, P>
where
    W: Sync + Fn(&[u8], &Float, u64, u64, u64, u32) -> Float,
    P: Sync + Fn(),
{
    /// Simulate the POS algorithm on all shards for the same height.
    pub fn simulate_height(&self, seed: &[u8], height: u64) -> Result {
        let mut shards_wins = vec![0; self.validators];
        let mut result = Result::new(self.validators, self.precision);

        for shard in 0..self.shards {
            let mut winner = 0;
            let mut winner_weight = Float::with_val(self.precision, Special::NegInfinity);

            for (validator, power) in self.powers.iter().enumerate() {
                let weight =
                    (self.weight)(seed, power, height, shard, validator as u64, self.precision);

                // println!("{}", weight);

                if weight > winner_weight {
                    winner = validator;
                    winner_weight = weight;
                }
            }

            // println!("{},{}", winner, &winner_weight);

            result.wins[winner] += 1;
            shards_wins[winner] += 1;

            result.min_win_weight.min_mut(&winner_weight);
            result.max_win_weight.max_mut(&winner_weight);
            result.sum_win_weight += winner_weight;
        }

        let max_wins = *shards_wins.iter().max().unwrap();
        if max_wins > result.max_win_across_shards {
            result.max_win_across_shards = max_wins;
        }

        result.rounds = self.shards;
        result
    }

    fn seed(epoch: u64) -> [u8; 32] {
        let mut hasher = Sha3_256::new();
        hasher.input(b"seed");
        hasher.input(epoch.to_be_bytes());
        hasher.result().into()
    }

    /// Simulate the POS algorithm on all shards for all blocks in given epoch.
    pub fn simulate_epoch(&self, epoch: u64) -> Result {
        let seed = Self::seed(epoch);

        (0..self.blocks_per_epoch)
            .into_par_iter()
            .map(|h| self.simulate_height(&seed, h))
            .inspect(|_| (self.progress)())
            .reduce(
                || Result::new(self.validators, self.precision),
                Result::merge,
            )
    }

    /// Siumate the POS algorithms on all shards for all blocks.
    pub fn simulate_full(&self) -> Result {
        (0..self.epochs)
            .into_par_iter()
            .map(|e| self.simulate_epoch(e))
            .reduce(
                || Result::new(self.validators, self.precision),
                Result::merge,
            )
    }
}

/// Compute a "random number".
pub fn random(seed: &[u8], height: u64, shard: u64, validator: u64, precision: u32) -> Float {
    // Generate "random" number.
    let mut hasher = Sha3_256::new();
    hasher.input(seed);
    hasher.input(shard.to_be_bytes());
    hasher.input(height.to_be_bytes());
    hasher.input(validator.to_be_bytes());
    let hash = hasher.result();

    let hash = Integer::from_digits(&hash, Order::Lsf);
    Float::with_val(precision, hash)
}

/// Weight forumula using a single exp.
pub fn weight_exp(
    seed: &[u8],
    power: &Float,
    height: u64,
    shard: u64,
    validator: u64,
    precision: u32,
) -> Float {
    let rand = random(seed, height, shard, validator, precision);

    // Transform number in interval [0;1].
    let hash_max = Float::with_val(precision, 2).pow(256);
    let rand: Float = rand / hash_max;

    // Compute weight.
    rand.pow(Float::with_val(precision, 1 / power))
}

pub fn weight_log(
    seed: &[u8],
    power: &Float,
    height: u64,
    shard: u64,
    validator: u64,
    precision: u32,
) -> Float {
    let rand = random(seed, height, shard, validator, precision);

    // Compute weight.
    let ln_r = rand.ln();
    let hash_max: Float = Float::with_val(precision, 2).pow(256);
    let ln_max = hash_max.clone().ln();
    let ln_d = Float::with_val(precision, 5).ln();

    (ln_r - ln_max) / (power * ln_d) // + hash_max
}
