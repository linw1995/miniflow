use mf_runtime::{PreparedStream, StreamClock, StreamError, StreamInstance, StreamOptions};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    task::Waker,
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Clock {
    time: Mutex<Duration>,
    wakers: Mutex<Vec<Waker>>,
}
impl StreamClock for Clock {
    fn now(&self) -> Duration {
        *self.time.lock().unwrap()
    }
    fn register_waker(&self, waker: Waker) {
        self.wakers.lock().unwrap().push(waker);
    }
}
impl Clock {
    fn advance(&self, milliseconds: u64) {
        *self.time.lock().unwrap() = Duration::from_millis(milliseconds);
        for waker in self.wakers.lock().unwrap().iter() {
            waker.wake_by_ref();
        }
    }
}
fn settle(instance: &StreamInstance, input: &mf_runtime::ChannelSender) -> Result<(), StreamError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(error) = instance.failure() {
            return Err(error);
        }
        let summary = instance.summary();
        if summary.completed_frames == summary.startup_frames + summary.emitted_messages
            && input.metrics().published == input.metrics().accepted
        {
            return Ok(());
        }
        assert!(Instant::now() < deadline, "frame processing stalled");
        thread::sleep(Duration::from_millis(1));
    }
}
fn receive(instance: &StreamInstance, outputs: &mut Vec<Value>) -> Result<(), StreamError> {
    outputs
        .push(serde_json::to_value(instance.recv()?.expect("expected a result").outputs).unwrap());
    Ok(())
}

pub fn drive(mut prepared: PreparedStream, scenario: &str) -> Value {
    let clock = Arc::new(Clock::default());
    let input = prepared.channel("feed").unwrap();
    let instance = prepared
        .start_with_options(StreamOptions {
            clock: clock.clone(),
            ..Default::default()
        })
        .unwrap();
    let mut outputs = Vec::new();
    let result = (|| -> Result<(), StreamError> {
        match scenario {
            "windows" => {
                for value in [1, 2] {
                    input.send(json!(value))?;
                    settle(&instance, &input)?;
                }
                clock.advance(100);
                receive(&instance, &mut outputs)?;
                for value in [3, 4, 5] {
                    input.send(json!(value))?;
                    settle(&instance, &input)?;
                }
                receive(&instance, &mut outputs)?;
                clock.advance(150);
                for value in [-1, 6] {
                    input.send(json!(value))?;
                    settle(&instance, &input)?;
                }
                // The count flush cancelled the old deadline at 200; this buffer is due at 250.
                clock.advance(200);
                input.send(json!(7))?;
                settle(&instance, &input)?;
            }
            "failure" => {
                input.send(json!(1))?;
                settle(&instance, &input)?;
                receive(&instance, &mut outputs)?;
                input.send(json!(2))?;
            }
            "chain" => {
                for value in 1..=5 {
                    input.send(json!(value))?;
                }
            }
            "nested" => {
                for value in 1..=3 {
                    input.send(json!(value))?;
                }
            }
            _ => panic!("unknown scenario"),
        }
        input.close();
        if scenario == "chain" {
            let deadline = Instant::now() + Duration::from_secs(10);
            while instance.summary().emitted_messages != 10 {
                if let Some(error) = instance.failure() {
                    return Err(error);
                }
                assert!(
                    Instant::now() < deadline,
                    "tail flush depended on output consumption"
                );
                thread::sleep(Duration::from_millis(1));
            }
        }
        while let Some(output) = instance.recv()? {
            outputs.push(serde_json::to_value(output.outputs).unwrap());
        }
        Ok(())
    })();
    let joined = instance.join();
    let error = result
        .err()
        .or_else(|| joined.err())
        .map(|error| error.to_string());
    json!({"outputs":outputs, "error":error})
}
