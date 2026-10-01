//! Compare retained JSON copies with retained persistent roots.
use mf_runtime::ValueRef;
use serde_json::json;
use std::{hint::black_box, time::Instant};

fn main() {
    let steps = 500;
    let original = json!({"payload": "x".repeat(64 * 1024), "counter": 0});
    for mode in ["unchanged", "partial", "replacement"] {
        let started = Instant::now();
        let mut owned = original.clone();
        let mut copies = Vec::new();
        for index in 0..steps {
            match mode {
                "partial" => owned["counter"] = json!(index),
                "replacement" => {
                    owned = json!({"payload": format!("{index:08}{}", "y".repeat(64 * 1024)), "counter": index})
                }
                _ => {}
            }
            copies.push(owned.clone());
        }
        black_box(&copies);
        let copy_us = started.elapsed().as_micros();
        let started = Instant::now();
        let mut shared = ValueRef::from(original.clone());
        let mut roots = Vec::new();
        for index in 0..steps {
            match mode {
                "partial" => shared = shared.with_field("counter", index).unwrap(),
                "replacement" => {
                    shared = ValueRef::from(
                        json!({"payload": format!("{index:08}{}", "y".repeat(64 * 1024)), "counter": index}),
                    )
                }
                _ => {}
            }
            roots.push(shared.clone());
        }
        black_box(&roots);
        let shared_us = started.elapsed().as_micros();
        println!(
            "{mode}: owned_us={copy_us} shared_us={shared_us} roots={}",
            roots.len()
        );
    }
}
