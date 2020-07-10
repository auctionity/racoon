// Cargo run --release

use racoon_weight::{weight_exp, Config};

use indicatif::{ProgressBar, ProgressStyle};
use rug::{integer::Order, ops::Pow, Float, Integer};
use sha3::{Digest, Sha3_256};

/// Amount of validators.
const VALIDATORS: usize = 1000;
/// Number of shards.
const SHARDS: u64 = 1;
/// Number of epochs.
const EPOCHS: u64 = 1;
/// Number of blocks in 1 epoch.
const HEIGHTS_PER_EPOCH: u64 = 1000;
/// Precisions in bits of the floating point numbers.
const FLOAT_PRECISION: u32 = 53;
/// Pread factor. Higher number will result in greater differencies between
/// biggest validators and the others.
const STAKE_SPREAD_FACTOR: u32 = 20;

fn main() {
    println!("Validators: {}", VALIDATORS);
    println!("Shards: {}", SHARDS);
    println!("Epochs: {}", EPOCHS);
    println!("Blocks per epoch: {}", HEIGHTS_PER_EPOCH);
    println!("Float precision: {}", FLOAT_PRECISION);
    println!("Stake spread factor: {}", STAKE_SPREAD_FACTOR);
    println!();

    let powers = powers();
    simulate(&powers, &weight_exp);
}

fn simulate<W>(powers: &[Float], formula: &W)
where
    W: Sync + Fn(&[u8], &Float, u64, u64, u64, u32) -> Float,
{
    let progress = ProgressBar::new((EPOCHS * HEIGHTS_PER_EPOCH) as u64);
    progress.set_style(
        ProgressStyle::default_bar()
            .template("Simulating POS: [{elapsed} - {eta}] [{wide_bar}] Height {pos}/{len}")
            .progress_chars("=> "),
    );

    let config = Config {
        powers: &powers,
        weight: formula,
        progress: || progress.inc(1),
        validators: VALIDATORS,
        shards: SHARDS,
        epochs: EPOCHS,
        blocks_per_epoch: HEIGHTS_PER_EPOCH,
        precision: FLOAT_PRECISION,
    };

    let result = config.simulate_full();

    progress.finish();
    println!();
    result.display(&powers, 10, FLOAT_PRECISION);
}

/// Generate the stake of a validator.
fn stake(id: u64) -> Float {
    let mut hasher = Sha3_256::new();
    hasher.input(b"validator");
    hasher.input(id.to_be_bytes());
    let stake = hasher.result();

    let stake = Integer::from_digits(&stake, Order::Lsf);
    let stake = Float::with_val(FLOAT_PRECISION, stake);

    let hash_max = Float::with_val(FLOAT_PRECISION, 2).pow(256);

    let stake: Float = stake / hash_max;
    let stake: Float = stake * 10;
    let stake = stake.pow(STAKE_SPREAD_FACTOR);

    1 + stake
}

/// Generate validators powers.
fn powers() -> Vec<Float> {
    let mut stakes = vec![Float::with_val(FLOAT_PRECISION, 0); VALIDATORS];
    let mut stakes_sum = Float::with_val(FLOAT_PRECISION, 0);

    // generate stakes and stakes sum
    for (i, s) in stakes.iter_mut().enumerate() {
        let stake = stake(i as u64);
        stakes_sum += stake.clone();
        *s = stake;
    }

    stakes.sort_by(|a, b| b.partial_cmp(a).unwrap()); // highest first
    stakes
        .into_iter()
        .map(|s| Float::with_val(FLOAT_PRECISION, s) / stakes_sum.clone())
        .collect()
}
