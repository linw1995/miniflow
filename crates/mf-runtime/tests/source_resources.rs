use mf_runtime::{ExecutionResources, StreamCancellation, StreamError, StreamInput, ValueType};
use serde_json::json;
use std::{fs::File, sync::mpsc, thread, time::Duration};

#[test]
fn channel_handles_close_independently_and_the_last_sender_closes_admission() {
    let mut resources = ExecutionResources::default();
    let first = resources.channel("first", ValueType::Int64, 2).unwrap();
    let second = resources.channel("second", ValueType::Int64, 2).unwrap();
    first.send(json!(1)).unwrap();
    second.send(json!(2)).unwrap();
    drop(first);
    assert_eq!(resources.channel_next("first").unwrap().unwrap(), json!(1));
    assert!(resources.channel_next("first").unwrap().is_none());
    second.send(json!(3)).unwrap();
    second.close();
    for value in [2, 3] {
        assert_eq!(
            resources.channel_next("second").unwrap().unwrap(),
            json!(value)
        );
    }
    assert!(resources.channel_next("second").unwrap().is_none());
}

#[test]
fn abandoning_prepared_resources_wakes_a_blocked_sender() {
    let mut resources = ExecutionResources::default();
    let sender = resources.channel("feed", ValueType::Int64, 1).unwrap();
    sender.send(json!(1)).unwrap();
    let watch = sender.clone();
    let (done, result) = mpsc::channel();
    let worker = thread::spawn(move || done.send(sender.send(json!(2))).unwrap());
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while watch.metrics().waiting_senders == 0 {
        assert!(std::time::Instant::now() < deadline);
        thread::yield_now();
    }
    drop(resources);
    assert!(matches!(
        result.recv_timeout(Duration::from_secs(2)).unwrap(),
        Err(StreamError::Closed)
    ));
    worker.join().unwrap();
}

#[test]
fn stdin_framing_preserves_values_and_reports_invalid_line_numbers() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("input");
    std::fs::write(&path, b"[1,2]\r\n[]\n[3]").unwrap();
    let cancellation = StreamCancellation::default();
    let mut input = StreamInput::new(File::open(&path).unwrap());
    let value_type = ValueType::List(Box::new(ValueType::Int64));
    for expected in [json!([1, 2]), json!([]), json!([3])] {
        assert_eq!(
            input
                .next_value(&value_type, &cancellation)
                .unwrap()
                .unwrap(),
            expected
        );
    }
    assert!(
        input
            .next_value(&value_type, &cancellation)
            .unwrap()
            .is_none()
    );
    for invalid in [
        b"1\n\n".as_slice(),
        b"1\nwrong",
        b"1\n\xff\n",
        b"1\n\"text\"\n",
    ] {
        std::fs::write(&path, invalid).unwrap();
        let mut input = StreamInput::new(File::open(&path).unwrap());
        assert_eq!(
            input
                .next_value(&ValueType::Int64, &cancellation)
                .unwrap()
                .unwrap(),
            json!(1)
        );
        assert!(
            input
                .next_value(&ValueType::Int64, &cancellation)
                .unwrap_err()
                .to_string()
                .contains("line 2")
        );
    }
}

#[cfg(unix)]
#[test]
fn cancellation_interrupts_stdin_without_another_byte_or_eof() {
    use std::os::fd::FromRawFd;
    let mut descriptors = [0; 2];
    assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
    let input = unsafe { File::from_raw_fd(descriptors[0]) };
    let writer = unsafe { File::from_raw_fd(descriptors[1]) };
    let cancellation = StreamCancellation::default();
    let worker_cancellation = cancellation.clone();
    let (done, result) = mpsc::channel();
    let worker = thread::spawn(move || {
        done.send(StreamInput::new(input).next_value(&ValueType::Any, &worker_cancellation))
            .unwrap()
    });
    cancellation.cancel(StreamError::Execution {
        message: "cancelled input".into(),
    });
    assert!(
        result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .is_err()
    );
    worker.join().unwrap();
    drop(writer);
}
