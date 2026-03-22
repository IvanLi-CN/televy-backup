use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn wait_for_exit(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    return false;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return false,
        }
    }
}

#[test]
fn shutdown_request_exits_cleanly() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_televybackup-mtproto-helper"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(stdout);

    stdin.write_all(br#"{"cmd":"shutdown"}"#).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();

    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(line.contains(r#""ok":true"#));

    drop(stdin);
    assert!(wait_for_exit(&mut child, Duration::from_secs(5)));
}

#[test]
fn eof_exits_cleanly() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_televybackup-mtproto-helper"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();

    drop(child.stdin.take());
    assert!(wait_for_exit(&mut child, Duration::from_secs(5)));
}
