use mf_runtime::{StreamCancellation, StreamError, TextInput};
use std::{fs::File, sync::mpsc, thread, time::Duration};

#[test]
fn text_lines_preserve_values_and_report_invalid_utf8_line_numbers() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("text");
    std::fs::write(&path, "[1,2]\r\n\n  text\t \n\u{4f60}\u{597d}\nfinal\r").unwrap();
    let cancellation = StreamCancellation::default();
    let mut input = TextInput::new(File::open(&path).unwrap());
    for expected in ["[1,2]", "", "  text\t ", "\u{4f60}\u{597d}", "final\r"] {
        assert_eq!(input.next_line(&cancellation).unwrap().unwrap(), expected);
    }
    assert!(input.next_line(&cancellation).unwrap().is_none());
    std::fs::write(&path, b"first\n\xff\n").unwrap();
    let mut input = TextInput::new(File::open(&path).unwrap());
    assert_eq!(input.next_line(&cancellation).unwrap().unwrap(), "first");
    assert!(
        input
            .next_line(&cancellation)
            .unwrap_err()
            .to_string()
            .contains("line 2")
    );
}

#[cfg(unix)]
#[test]
fn cancellation_interrupts_idle_text_input() {
    use std::os::fd::FromRawFd;
    let mut descriptors = [0; 2];
    assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
    let input = unsafe { File::from_raw_fd(descriptors[0]) };
    let writer = unsafe { File::from_raw_fd(descriptors[1]) };
    let cancellation = StreamCancellation::default();
    let worker_cancellation = cancellation.clone();
    let (done, result) = mpsc::channel();
    let worker = thread::spawn(move || {
        done.send(TextInput::new(input).next_line(&worker_cancellation))
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
