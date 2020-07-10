use rug::{integer::Order, ops::Pow, Float, Integer};
use serde::Deserialize;
use sha3::{Digest, Sha3_256};
use std::{
    cmp::{Ord, Ordering},
    collections::{BinaryHeap, HashMap},
    fs::File,
};

/// Precisions in bits of the floating point numbers.
const FLOAT_PRECISION: u32 = 53;

#[derive(Clone, Debug, Deserialize)]
struct Config {
    validators_count: usize,
    heights_per_epoch: u64,
    stake_spread_factor: u64,
    block_time: u64,
    max_weight_time: u64,
    finalization_weight: u64,
    latency: u64,
    lucky_retry: u64,
    stop_height: u64,
}

#[allow(clippy::cognitive_complexity)]
fn main() {
    simple_logger::init_with_level(log::Level::Info).unwrap();
    // simple_logger::init().unwrap();
    let config: Config = ron::de::from_reader(File::open("config.ron").unwrap()).unwrap();

    log::debug!("Config : {:#?}", config);

    let mut event_queue = BinaryHeap::new();
    let mut validators: Vec<_> = powers(config.validators_count, config.stake_spread_factor)
        .into_iter()
        .inspect(|power| log::debug!("Registered validator with power {}", power))
        .map(Validator::from_power)
        .collect();

    let mut next_block_id = 1; // 0 is genesis
    let mut blocks = HashMap::<u64, Block>::new();
    let mut finalized_blocks = HashMap::<u64, u64>::new();

    log::info!("Starting event loop ...");

    for i in 0..validators.len() {
        event_queue.push(TimedEvent {
            time: 0,
            target: i,
            event: Event::BlockReceived { block: 0 },
        })
    }

    'event_loop: while let Some(event) = event_queue.pop() {
        // println!();
        log::trace!("---------- Event : {:?}", event);

        let TimedEvent {
            time,
            target,
            event,
        } = event;

        match event {
            Event::BlockReceived { block } => {
                if block == 0 {
                    log::trace!("Genesis block");

                    start_vdf(
                        &config,
                        &mut event_queue,
                        time + config.block_time,
                        0,
                        0,
                        1,
                        target,
                        &validators[target].power,
                    );

                    start_vdf(
                        &config,
                        &mut event_queue,
                        time + config.block_time * 2,
                        0,
                        0,
                        2,
                        target,
                        &validators[target].power,
                    );
                } else {
                    log::trace!("Received block #{} : {:?}", block, blocks[&block]);

                    let validator = &mut validators[target];

                    let (mut fork_weight, mut maybe_finalizable_block) =
                        match branch_weight(&blocks, validator.finalized_block, block) {
                            Some(res) => res,
                            None => {
                                log::warn!(
                                    "Received fork not containing finalized block, ignoring ..."
                                );
                                continue;
                            }
                        };

                    log::trace!(
                        "Current fork weight sum : {}",
                        validator.current_fork_weight
                    );
                    log::trace!("Received fork weight sum : {}", fork_weight);

                    if fork_weight > validator.current_fork_weight {
                        log::debug!("Switch to received fork (head #{})", block);

                        validator.current_head = block;
                        validator.current_fork_weight = fork_weight.clone();

                        if blocks[&block].height < config.stop_height {
                            start_vdf(
                                &config,
                                &mut event_queue,
                                time + config.block_time * 2,
                                0,
                                block,
                                blocks[&block].height + 2,
                                target,
                                &validators[target].power,
                            );
                        }

                        while fork_weight > config.finalization_weight {
                            let validator = &mut validators[target];
                            let finalized_height = blocks[&maybe_finalizable_block].height;

                            log::info!(
                                "Validator {} finalized block #{} (height: {})",
                                target,
                                maybe_finalizable_block,
                                finalized_height
                            );

                            if let Some(other_finalized) = finalized_blocks.get(&finalized_height) {
                                if *other_finalized != maybe_finalizable_block {
                                    log::error!("FINALIZATION DIVERGENCE");
                                    break 'event_loop;
                                }
                            } else {
                                finalized_blocks.insert(finalized_height, maybe_finalizable_block);
                            }

                            validator.finalized_block = maybe_finalizable_block;

                            let res =
                                branch_weight(&blocks, validator.finalized_block, block).unwrap();
                            fork_weight = res.0;
                            maybe_finalizable_block = res.1;

                            validator.current_fork_weight = fork_weight.clone();
                        }
                    } else {
                        log::trace!("Staying on current fork (head #{})", validator.current_head);
                    }
                }
            }
            Event::VdfFinished {
                input_block,
                output_height,
                weight,
            } => {
                if input_block == 0 {
                    let previous_block = if validators[target].current_head == 0 {
                        0
                    } else {
                        1
                    };

                    log::trace!(
                        "Current head #{} (height {})",
                        validators[target].current_head,
                        previous_block
                    );

                    // let block_head = &blocks[&validators[target].current_head];

                    let block = Block {
                        height: output_height,
                        previous_block,
                        validator_id: target,
                        shard_id: 0,
                        weight,
                    };

                    log::trace!("Pushed block #{} : {:?}", next_block_id, block);
                    blocks.insert(next_block_id, block);

                    for i in 0..validators.len() {
                        let latency = if target == i { 0 } else { config.latency };

                        event_queue.push(TimedEvent {
                            time: time + latency,
                            target: i,
                            event: Event::BlockReceived {
                                block: next_block_id,
                            },
                        })
                    }

                    next_block_id += 1;
                } else {
                    let block_head = &blocks[&validators[target].current_head];

                    log::trace!(
                        "Current head #{} (height {})",
                        validators[target].current_head,
                        block_head.height
                    );

                    if block_head.previous_block == input_block {
                        // log::error!(
                        //     "TODO : Create block from VDF (height {})",
                        //     block_head.height + 1
                        // );

                        let block = Block {
                            height: output_height,
                            previous_block: validators[target].current_head,
                            validator_id: target,
                            shard_id: 0,
                            weight: weight.clone(),
                        };

                        log::trace!("Pushed block #{} : {:?}", next_block_id, block);
                        blocks.insert(next_block_id, block);

                        for i in 0..validators.len() {
                            let latency = if target == i { 0 } else { config.latency };

                            event_queue.push(TimedEvent {
                                time: time + latency,
                                target: i,
                                event: Event::BlockReceived {
                                    block: next_block_id,
                                },
                            });
                        }

                        next_block_id += 1;
                    } else if validators[target].current_head == input_block {
                        log::debug!(
                            "VDF based on #{} arrived while it's still the head of validator {} (lucky next height), retrying soon ...",
                            input_block,
                            target,
                        );

                        event_queue.push(TimedEvent {
                            time: time + config.lucky_retry,
                            target: target,
                            event: Event::VdfFinished {
                                input_block,
                                output_height,
                                weight,
                            },
                        });
                    } else {
                        log::trace!(
                            "VDF based on #{} while head parent is #{}, ignoring ...",
                            input_block,
                            block_head.previous_block
                        );
                    }
                }
            }
        }
    }

    println!();
    log::info!("No more events, stopping !");

    // log::debug!("Blocks : {:#?}", blocks);
    log::info!("Validators : {:#?}", validators);
}

fn start_vdf(
    config: &Config,
    event_queue: &mut BinaryHeap<TimedEvent>,
    time: u64,
    shard_id: u64,
    vdf_input_block: u64,
    vdf_output_height: u64,
    validator: usize,
    validator_power: &Float,
) {
    let weight = block_weight(
        b"seed",
        shard_id,
        vdf_output_height,
        validator,
        validator_power,
    );

    log::trace!("weight = {}", weight);

    let vdf_time = config.max_weight_time
        - (config.max_weight_time * weight.clone())
            .to_u32_saturating()
            .unwrap() as u64;
    log::trace!(
        "Starting VDF of {} ticks (ending at {})",
        vdf_time,
        time + vdf_time
    );

    event_queue.push(TimedEvent {
        time: time + vdf_time,
        target: validator,
        event: Event::VdfFinished {
            input_block: vdf_input_block,
            output_height: vdf_output_height,
            weight,
        },
    });
}

fn branch_weight(
    blocks: &HashMap<u64, Block>,
    finalized: u64,
    mut head: u64,
) -> Option<(Float, u64)> {
    let mut weight_sum = Float::with_val(FLOAT_PRECISION, 0);

    let mut last_block = head;

    loop {
        if head == finalized {
            break;
        }

        if head == 0 {
            return None; // wrong fork
        }

        let block = &blocks[&head];
        weight_sum += block.weight.clone();

        last_block = head;
        head = block.previous_block;
    }

    Some((weight_sum, last_block))
}

#[derive(Debug, Clone)]
struct TimedEvent {
    time: u64,
    target: usize,
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

#[derive(Debug, Clone)]
enum Event {
    BlockReceived {
        block: u64,
    },
    VdfFinished {
        input_block: u64,
        output_height: u64,
        weight: Float,
    },
}

#[derive(Debug, Clone)]
struct Block {
    height: u64,
    previous_block: u64,
    validator_id: usize,
    shard_id: u64,
    weight: Float,
}

#[derive(Debug, Clone)]
struct Validator {
    power: Float,
    finalized_block: u64,
    current_head: u64,
    current_fork_weight: Float,
}

impl Validator {
    fn from_power(power: Float) -> Self {
        Validator {
            power,
            finalized_block: 0,
            current_head: 0,
            current_fork_weight: Float::with_val(FLOAT_PRECISION, 0),
        }
    }
}

fn block_random(epoch_seed: &[u8], shard_id: u64, block_height: u64, validator_id: u64) -> Float {
    let mut hasher = Sha3_256::new();
    hasher.input(epoch_seed);
    hasher.input(shard_id.to_be_bytes());
    hasher.input(block_height.to_be_bytes());
    hasher.input(validator_id.to_be_bytes());

    let hash = hasher.result();
    let hash = Integer::from_digits(&hash, Order::Lsf);
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
