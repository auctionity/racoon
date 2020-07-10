use rug::{integer::Order, ops::Pow, Float, Integer};
use serde::Deserialize;
use sha3::{Digest, Sha3_256};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BinaryHeap},
    fs::File,
};
use tracing::instrument;

/// Precisions in bits of the floating point numbers.
const FLOAT_PRECISION: u32 = 53;

/// Binary config.
#[derive(Clone, Debug, Deserialize)]
struct Config {
    /// Amount of validators.
    validators_count: usize,
    /// How much stakes are heterogenous between validators.
    stake_spread_factor: u64,

    /// Number of VDF ticks between 2 consecutive blocks.
    vdf_block_ticks: u64,
    /// Max number of VDF ticks that can be added relative to the block weight.
    vdf_max_weight_ticks: u64,
    /// Number of ticks a message from a validator takes to reach another one.
    latency_ticks: u64,
    /// Number of ticks a validator waits to try again a valid early VDF.
    vdf_apply_retry_ticks: u64,

    /// Cumulative weight necessary to finalize a block.
    finalization_weight: u64,
    /// Height at which a validator stops producing blocks (to stop the simulation).
    stop_height: u64,
    /// Step at which simulation is stopped. None don't stop. Usefull when debugging.
    step_stop: Option<u64>,
}

/// A simulation event.
#[derive(Debug, Clone)]
enum Event {
    /// A block has been received by the validator.
    BlockReceived { block_id: u64 },
    /// The validator has finished a VDF.
    VdfFinished {
        input_block_id: u64,
        output_block_height: u64,
        weight: Float,
    },
}

/// A timed simulation event.
#[derive(Debug, Clone)]
struct TimedEvent {
    /// Time at which an event occurs.
    time: u64,
    /// Validator reacting to this event.
    validator_id: usize,
    /// Simulation events.
    event: Event,
}

impl PartialEq for TimedEvent {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time
    }
}

impl Eq for TimedEvent {}

impl PartialOrd for TimedEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        other.time.partial_cmp(&self.time)
    }
}

impl Ord for TimedEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        other.time.cmp(&self.time)
    }
}

/// A validator.
#[derive(Debug, Clone)]
struct Validator {
    /// Validator power (% of stake).
    power: Float,
    /// Current finalized block (won't reorg to a fork not containing this block).
    finalized_block_id: u64,
    /// Current fork head.
    current_head_id: u64,
    /// Current fork cumulative weight.
    current_fork_weight: Float,
    /// ID of blocks on which VDF have been finished with their N+2 block weight.
    finished_vdf: BTreeMap<u64, Float>,

    latest_created_height: u64,
}

impl Validator {
    fn from_power(power: Float) -> Self {
        Validator {
            power,
            finalized_block_id: 0,
            current_head_id: 0,
            current_fork_weight: Float::with_val(FLOAT_PRECISION, 0),
            finished_vdf: BTreeMap::new(),
            latest_created_height: 0,
        }
    }
}

#[derive(Debug, Clone)]
struct Block {
    height: u64,
    previous_block_id: u64,
    validator_id: usize,
    // shard_id: u64,
    weight: Float,
    time: u64,
}

/// Simulation state.
#[derive(Debug, Clone)]
struct Simulation {
    /// Simulation config.
    config: Config,
    /// Pool of events. They will be executed in order of their `time`.
    event_pool: BinaryHeap<TimedEvent>,
    /// List of validators.
    validators: Vec<Validator>,
    /// Next free ID for block creation.
    next_free_block_id: u64,
    /// Map of id -> block data.
    blocks: BTreeMap<u64, Block>,
    /// Map of finalized blocks.
    finalized_blocks: BTreeMap<u64, u64>,

    stop: bool,

    progress: indicatif::ProgressBar,
}

impl Simulation {
    fn new(config: Config) -> Self {
        let validators: Vec<_> = powers(config.validators_count, config.stake_spread_factor)
            .into_iter()
            .map(Validator::from_power)
            .collect();

        let mut event_pool = BinaryHeap::new();

        for i in 0..validators.len() {
            event_pool.push(TimedEvent {
                time: 0,
                validator_id: i,
                event: Event::BlockReceived { block_id: 0 },
            })
        }

        let progress = indicatif::ProgressBar::new(config.stop_height);
        // let progress = indicatif::ProgressBar::hidden();
        progress.set_style(
            indicatif::ProgressStyle::default_bar()
                .template(
                    "[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} ({eta} remaining)",
                )
                .progress_chars("##-"),
        );

        Self {
            progress,
            config,
            event_pool,
            validators,
            next_free_block_id: 1, // 0 is genesis and special case.
            blocks: BTreeMap::new(),
            finalized_blocks: BTreeMap::new(),
            stop: false,
        }
    }

    fn run(&mut self) {
        tracing::trace!("Running simulation ...");
        for i in 0.. {
            if let Some(stop) = self.config.step_stop {
                if i > stop {
                    tracing::info!("Reached stop step");
                    break;
                }
            }

            let span = tracing::info_span!("step", step = i);
            let _guard = span.enter();

            if !self.next() || self.stop {
                tracing::info!("Event treatement asked to stop");
                break;
            }
        }
    }

    fn next(&mut self) -> bool {
        if let Some(event) = self.event_pool.pop() {
            self.process_event(event);
            true
        } else {
            false
        }
    }

    // #[instrument(skip(self))]
    fn process_event(&mut self, event: TimedEvent) {
        let TimedEvent {
            time,
            validator_id,
            event,
        } = event;

        match event {
            Event::BlockReceived { block_id } => {
                self.process_event_block_received(time, validator_id, block_id)
            }
            Event::VdfFinished {
                input_block_id,
                output_block_height,
                weight,
            } => self.process_event_vdf_finished(
                time,
                validator_id,
                input_block_id,
                output_block_height,
                weight,
            ),
        }
    }

    #[instrument(skip(self))]
    fn process_event_block_received(&mut self, time: u64, validator_id: usize, block_id: u64) {
        if block_id == 0 {
            self.start_vdf(time, 0, 1, validator_id);
            self.start_vdf(time, 0, 2, validator_id);
        } else {
            // Check block legitimacy.
            let validator_final_block_height =
                if self.validators[validator_id].finalized_block_id == 0 {
                    0
                } else {
                    self.blocks[&self.validators[validator_id].finalized_block_id].height
                };

            let (mut weight, mut maybe_finalizable_id) = match self.compute_fork_weight(
                block_id,
                self.validators[validator_id].finalized_block_id,
                validator_final_block_height,
            ) {
                Some(res) => res,
                None => {
                    tracing::warn!(
                        "Proposed block is not based on the validator finalized block, ignoring"
                    );
                    return;
                }
            };

            // Start VDF (on all forks to avoid halting after reorgs)
            if self.blocks[&block_id].height < self.config.stop_height {
                self.start_vdf(
                    time,
                    block_id,
                    self.blocks[&block_id].height + 2,
                    validator_id,
                );
            } else {
                tracing::debug!("Reached stop height");
            }

            if weight <= self.validators[validator_id].current_fork_weight {
                tracing::trace!(
                    previous_head = self.validators[validator_id].current_head_id,
                    previous_sum = %self.validators[validator_id].current_fork_weight,
                    proposed_head = block_id,
                    proposed_sum = %weight,
                    "Refused proposed head"
                );
                return;
            }

            tracing::trace!(
                previous_head = self.validators[validator_id].current_head_id,
                previous_sum = %self.validators[validator_id].current_fork_weight,
                new_head = block_id,
                new_sum = %weight,
                "Accepting new head"
            );

            // Accept block.
            self.validators[validator_id].current_head_id = block_id;
            self.validators[validator_id].current_fork_weight = weight.clone();

            // Create next block if next head parent already finished its VDF.
            if let Some(weight) = self.validators[validator_id]
                .finished_vdf
                .get(&self.blocks[&block_id].previous_block_id)
            {
                let weight = weight.clone();

                self.create_block(
                    time,
                    validator_id,
                    self.blocks[&block_id].height + 1,
                    block_id,
                    weight,
                );
            }

            // Finalize.
            while weight > self.config.finalization_weight {
                let finalized_height = self.blocks[&maybe_finalizable_id].height;

                tracing::debug!(
                    validator_id,
                    block_id = maybe_finalizable_id,
                    block_height = finalized_height,
                    "Finalized block"
                );

                self.progress.set_position(finalized_height);

                if let Some(other_finalized) = self.finalized_blocks.get(&finalized_height) {
                    if *other_finalized != maybe_finalizable_id {
                        tracing::error!("FINALIZATION DIVERGENCE");
                        self.stop = true;
                        return;
                    }
                } else {
                    self.finalized_blocks
                        .insert(finalized_height, maybe_finalizable_id);
                }

                self.validators[validator_id].finalized_block_id = maybe_finalizable_id;

                let res = match self.compute_fork_weight(
                    block_id,
                    maybe_finalizable_id,
                    finalized_height,
                ) {
                    Some(res) => res,
                    None => {
                        tracing::error!(%weight, maybe_finalizable_id, "shouldn't happend");
                        return;
                    }
                };

                weight = res.0;
                maybe_finalizable_id = res.1;

                self.validators[validator_id].current_fork_weight = weight.clone();
            }
        }
    }

    #[instrument(skip(self))]
    fn process_event_vdf_finished(
        &mut self,
        time: u64,
        validator_id: usize,
        input_block_id: u64,
        output_block_height: u64,
        weight: Float,
    ) {
        self.validators[validator_id]
            .finished_vdf
            .insert(input_block_id, weight.clone());

        let head_id = self.validators[validator_id].current_head_id;

        if input_block_id == 0 {
            if output_block_height == 1 {
                tracing::trace!("Creating block on top of genesis");
                self.create_block(time, validator_id, output_block_height, 0, weight);
                return;
            }

            if output_block_height == 2 {
                if head_id == 0 {
                    tracing::error!(head_id, "should not happend");
                    return;
                }

                let head_block = &self.blocks[&head_id];

                if head_block.previous_block_id != 0 {
                    tracing::trace!("Trying to use genesis VDF on wrong block, ignoring ...");
                    return;
                }

                tracing::trace!("Creating block on top of genesis child");
                self.create_block(time, validator_id, output_block_height, head_id, weight);
                return;
            }
        }

        if head_id == input_block_id {
            tracing::warn!(head_id, "Vinished VDF can't be used yet, trying later");

            let event = Event::VdfFinished {
                input_block_id,
                output_block_height,
                weight,
            };

            self.event_pool.push(TimedEvent {
                time: time + self.config.vdf_apply_retry_ticks,
                validator_id,
                event,
            });

            return;
        }

        let head_block = &self.blocks[&head_id];

        if head_block.previous_block_id != input_block_id {
            tracing::trace!(
                head_id,
                head_block.previous_block_id,
                "Vinished VDF that can't be used on current head"
            );
            return;
        }

        self.create_block(time, validator_id, output_block_height, head_id, weight);
    }

    #[instrument(skip(self))]
    fn compute_fork_weight(
        &self,
        mut block_head_id: u64,
        block_finalized_id: u64,
        block_finalized_height: u64,
    ) -> Option<(Float, u64)> {
        let mut weight_sum = Float::with_val(FLOAT_PRECISION, 0);
        let mut maybe_finalizable_id = block_head_id;

        loop {
            // tracing::trace!(block_head_id, maybe_finalizable_id, %weight_sum);

            if block_head_id == 0 {
                break;
            }

            let block_head = &self.blocks[&block_head_id];

            if block_head.height == block_finalized_height {
                if block_head_id == block_finalized_id {
                    break;
                } else {
                    // fork with divergent finalized block
                    return None;
                }
            }

            weight_sum += &block_head.weight;
            maybe_finalizable_id = block_head_id;
            block_head_id = block_head.previous_block_id;
        }

        Some((weight_sum, maybe_finalizable_id))
    }

    #[instrument(skip(self))]
    fn create_block(
        &mut self,
        time: u64,
        validator_id: usize,
        height: u64,
        previous_block_id: u64,
        weight: Float,
    ) {
        let latest_created_height = self.validators[validator_id].latest_created_height;
        if height <= latest_created_height {
            tracing::trace!(
                latest_created_height,
                "Validator can no longer create a block at this height"
            );
            return;
        }

        self.validators[validator_id].latest_created_height = latest_created_height;

        let block = Block {
            height,
            previous_block_id,
            validator_id,
            weight,
            time,
        };

        tracing::trace!("Pushed block #{} : {:?}", self.next_free_block_id, block);
        self.blocks.insert(self.next_free_block_id, block);

        for i in 0..self.validators.len() {
            let latency = if validator_id == i {
                0
            } else {
                self.config.latency_ticks
            };

            self.event_pool.push(TimedEvent {
                time: time + latency,
                validator_id: i,
                event: Event::BlockReceived {
                    block_id: self.next_free_block_id,
                },
            })
        }

        self.next_free_block_id += 1;
    }

    #[instrument(skip(self, current_time))]
    fn start_vdf(
        &mut self,
        current_time: u64,
        input_block_id: u64,
        output_block_height: u64,
        validator_id: usize,
    ) {
        let weight = block_weight(
            b"seed",
            0,
            output_block_height,
            validator_id,
            &self.validators[validator_id].power,
        );

        let vdf_blocks_length = if input_block_id == 0 {
            output_block_height
        } else {
            2
        };

        let base_ticks = vdf_blocks_length * self.config.vdf_block_ticks;
        let weight_ticks: Float = self.config.vdf_max_weight_ticks * (1 - weight.clone());
        let weight_ticks = weight_ticks.to_u32_saturating().unwrap() as u64;
        let vdf_ticks = base_ticks + weight_ticks;

        tracing::trace!(%weight, end_time = current_time + vdf_ticks, "Schedule VDF");

        let event = Event::VdfFinished {
            input_block_id,
            output_block_height,
            weight,
        };

        self.event_pool.push(TimedEvent {
            time: current_time + vdf_ticks,
            validator_id,
            event,
        });
    }

    #[instrument(skip(self))]
    pub fn print_stats(&self) {
        self.progress.finish();

        self.print_fairness();
        self.print_average_time();
    }

    fn print_fairness(&self) {
        let mut validators_wins = vec![0; self.config.validators_count];

        for (_height, block_id) in &self.finalized_blocks {
            let block = &self.blocks[&block_id];
            validators_wins[block.validator_id] += 1;
        }

        let mut diff_sum = 0.0;

        for i in 0..self.config.validators_count {
            let winrate = validators_wins[i] as f64 / self.finalized_blocks.len() as f64;
            let power = self.validators[i].power.to_f64();
            let diff = winrate - power;
            diff_sum += diff.abs();
        }

        let fairness = diff_sum / self.config.validators_count as f64;

        println!("Fairness : {:.9}", fairness);
    }

    fn print_average_time(&self) {
        let mut diff_sum = 0.0;
        let mut diff_min = std::u64::MAX;
        let mut diff_max = 0;

        let mut diff_odd_even_sum = 0.0;
        let mut diff_odd_even_min = std::u64::MAX;
        let mut diff_odd_even_max = 0;

        let mut diff_even_odd_sum = 0.0;
        let mut diff_even_odd_min = std::u64::MAX;
        let mut diff_even_odd_max = 0;

        let mut odd_even_count = 0;
        let mut even_odd_count = 0;

        let mut iter = self.finalized_blocks.iter().peekable();

        while let Some((height, block_id)) = iter.next() {
            if let Some((_, next_block_id)) = iter.peek() {
                let block0 = &self.blocks[block_id];
                let block1 = &self.blocks[next_block_id];
                let diff = block1.time - block0.time;
                diff_sum += diff as f64;
                diff_min = std::cmp::min(diff_min, diff);
                diff_max = std::cmp::max(diff_max, diff);

                if height % 2 == 0 {
                    diff_even_odd_sum += diff as f64;
                    diff_even_odd_min = std::cmp::min(diff_even_odd_min, diff);
                    diff_even_odd_max = std::cmp::max(diff_even_odd_max, diff);
                    even_odd_count += 1;
                } else {
                    diff_odd_even_sum += diff as f64;
                    diff_odd_even_min = std::cmp::min(diff_odd_even_min, diff);
                    diff_odd_even_max = std::cmp::max(diff_odd_even_max, diff);
                    odd_even_count += 1;
                }
            }
        }

        let average_time = diff_sum / (self.finalized_blocks.len() - 1) as f64;
        let average_even_odd = diff_even_odd_sum / even_odd_count as f64;
        let average_odd_even = diff_odd_even_sum / odd_even_count as f64;

        println!("Average block time : {:.1}", average_time);
        println!("Min block time : {}", diff_min);
        println!("Max block time : {}", diff_max);

        println!("Average even-odd block time : {:.1}", average_even_odd);
        println!("Min even-odd block time : {}", diff_even_odd_min);
        println!("Max even-odd block time : {}", diff_even_odd_max);

        println!("Average odd-even block time : {:.1}", average_odd_even);
        println!("Min odd-even block time : {}", diff_odd_even_min);
        println!("Max odd-even block time : {}", diff_odd_even_max);
    }
}

fn main() {
    init_tracing();
    let config = config_from_ron_file("config.ron");

    let mut simulation = Simulation::new(config);

    simulation.run();
    simulation.print_stats();
}

fn init_tracing() {
    use tracing_subscriber::field::MakeExt;

    tracing_subscriber::FmtSubscriber::builder()
        .without_time()
        .with_target(false)
        // .with_ansi(false)
        // .json()
        .fmt_fields(
            tracing_subscriber::fmt::format::debug_fn(|writer, field, value| {
                write!(writer, "{}: {:?}", field, value)
            })
            // Use the `tracing_subscriber::MakeFmtExt` trait to wrap the
            // formatter so that a delimiter is added between fields.
            .delimited(", "),
        )
        .with_max_level(tracing::Level::INFO)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
}

fn config_from_ron_file(path: &str) -> Config {
    let file = File::open(path).unwrap();
    ron::de::from_reader(file).unwrap()
}

fn block_random(epoch_seed: &[u8], shard_id: u64, block_height: u64, validator_id: u64) -> Float {
    // let mut hasher = Sha3_256::new();
    // hasher.input(epoch_seed);
    // hasher.input(shard_id.to_be_bytes());
    // hasher.input(block_height.to_be_bytes());
    // hasher.input(validator_id.to_be_bytes());

    // let hash = hasher.result();
    // let hash = Integer::from_digits(&hash, Order::Lsf);

    let mut hasher = blake3::Hasher::new();
    hasher.update(epoch_seed);
    hasher.update(&shard_id.to_be_bytes());
    hasher.update(&block_height.to_be_bytes());
    hasher.update(&validator_id.to_be_bytes());
    let hash = hasher.finalize();
    let hash = Integer::from_digits(hash.as_bytes(), Order::Lsf);

    Float::with_val(FLOAT_PRECISION, hash)
}

fn block_weight(
    epoch_seed: &[u8],
    shard_id: u64,
    block_height: u64,
    validator_id: usize,
    validator_power: &Float,
) -> Float {
    let rand = block_random(epoch_seed, shard_id, block_height, validator_id as u64);

    let max = Float::with_val(FLOAT_PRECISION, 2).pow(256);
    let rand: Float = rand / max;

    rand.pow(Float::with_val(FLOAT_PRECISION, 1 / validator_power))
}

fn powers(count: usize, spread_factor: u64) -> Vec<Float> {
    let mut stakes = vec![Float::with_val(FLOAT_PRECISION, 0); count];
    let mut stakes_sum = Float::with_val(FLOAT_PRECISION, 0);

    // generate stakes and stakes sum
    for (i, s) in stakes.iter_mut().enumerate() {
        let stake = stake(i as u64, spread_factor);
        stakes_sum += stake.clone();
        *s = stake;
    }

    stakes.sort_by(|a, b| b.partial_cmp(a).unwrap()); // highest first
    stakes
        .into_iter()
        .map(|s| Float::with_val(FLOAT_PRECISION, s) / stakes_sum.clone())
        .collect()
}

fn stake(id: u64, spread_factor: u64) -> Float {
    let mut hasher = Sha3_256::new();
    hasher.input(b"validator");
    hasher.input(id.to_be_bytes());
    let stake = hasher.result();

    let stake = Integer::from_digits(&stake, Order::Lsf);
    let stake = Float::with_val(FLOAT_PRECISION, stake);

    let hash_max = Float::with_val(FLOAT_PRECISION, 2).pow(256);

    let stake: Float = stake / hash_max;
    let stake: Float = stake * 10;
    let stake = stake.pow(spread_factor);

    1 + stake
}
