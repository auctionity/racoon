use racoon_weight::weight_exp;
use rayon::prelude::*;
use rug::Float; use rug::ops::Pow;

fn main() {
    println!("Stake,Takeover Rate");
    for i in 0..60 {
        let power = i as f64 * 0.01;
        let take_over_rate = compute_selfish_rate(power);
        // let second_rate = compute_selfish_rate(power, false);
        println!("{},{}", power, take_over_rate,);
    }
}

fn compute_selfish_rate(attacker_power: f64) -> f64 {
    let precision = 53;
    let attacker_power = Float::with_val(precision, attacker_power);
    let tries = 1_000;

    // let selfish_count = (0..tries)
    //     .filter(|i| can_be_selfish(format!("seed:{}", i).as_bytes(), &attacker_power, precision, first_genuine))
    //     .count();

    let selfish_attemps = (0..tries)
        .into_par_iter()
        .filter_map(|i| {
            can_be_selfish(format!("seed:{}", i).as_bytes(), &attacker_power, precision)
        })
        .collect::<Vec<_>>();

    let tries = selfish_attemps.len();
    let selfish_count = selfish_attemps.iter().filter(|&a| *a).count();

    selfish_count as f64 / tries as f64
}

fn can_be_selfish(seed: &[u8], attacker_power: &Float, precision: u32) -> Option<bool> {
    let genuine_power = Float::with_val(precision, 1) - attacker_power;

    let attacker_block_0 = weight_exp_correct(seed, attacker_power, 0, 0, 0, precision);
    let genuine_block_0 = weight_exp_correct(seed, &genuine_power, 0, 0, 1, precision);

    if genuine_block_0 <= attacker_block_0 {
        return None; // attacker fairly wins, ignoring
    }

    let attacker_block_1 = weight_exp_correct(seed, attacker_power, 1, 0, 0, precision);
    let genuine_block_1 = weight_exp_correct(seed, &genuine_power, 1, 0, 1, precision);

    let attacker_block_sum = attacker_block_0 + attacker_block_1;
    let genuine_block_sum = genuine_block_0 + genuine_block_1;

    Some(attacker_block_sum > genuine_block_sum)
}

pub fn weight_exp_correct(
    seed: &[u8],
    power: &Float,
    height: u64,
    shard: u64,
    validator: u64,
    precision: u32,
) -> Float {
    let weight = weight_exp(seed, power, height, shard, validator, precision);

    // weight.pow(Float::with_val(precision, 10000))
    // Float::with_val(precision, 0)
    weight
}
