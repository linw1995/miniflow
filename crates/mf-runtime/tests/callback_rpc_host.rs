#![cfg(all(unix, feature = "callback-host"))]

use mf_runtime::{
    StreamCancellation,
    callback::{CallbackSites, HostCallback, HostCallbacks},
    callback_rpc::{Grant, LAUNCH_ENV, host::Listener},
};
use std::{
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

fn grant(command: &Command) -> Grant {
    let encoded = command
        .get_envs()
        .find(|(key, _)| *key == LAUNCH_ENV)
        .unwrap()
        .1
        .unwrap();
    serde_json::from_str(encoded.to_str().unwrap()).unwrap()
}

#[test]
fn empty_grants_clear_inherited_authority_and_unknown_sites_fail_before_spawn() {
    let mut command = Command::new("true");
    command.env(LAUNCH_ENV, "inherited");
    let sites = CallbackSites::new();
    assert!(
        Listener::prepare(&mut command, &sites, Default::default())
            .unwrap()
            .is_none()
    );
    assert!(
        command
            .get_envs()
            .any(|(key, val)| key == LAUNCH_ENV && val.is_none())
    );
    let handlers = [("unknown".into(), HostCallback::new(|_, _| unreachable!()))].into();
    assert!(Listener::prepare(&mut command, &sites, handlers).is_err());
}

#[test]
fn the_runtime_authenticates_the_launched_pid_and_owns_the_listener_lifetime() {
    let sites = [("lookup".into(), vec!["site".into()])].into();
    let (sent, received) = mpsc::channel();
    let handlers: HostCallbacks = [(
        "lookup".into(),
        HostCallback::new(move |args, _| {
            sent.send(args.0["key"].as_str().unwrap().to_owned())
                .unwrap();
            Ok(mf_runtime::Outputs::from([("value".into(), "bound".into())]).into())
        }),
    )]
    .into();
    let mut command = Command::new("python3");
    command.args(["-c", r#"
import json, os, socket, stat
launch = json.loads(os.environ['MF_RUNNER_RPC_V1'])
path = launch['socket']
assert launch['parent'] == os.getppid() and launch['bindings'] == ['lookup']
assert stat.S_IMODE(os.stat(os.path.dirname(path)).st_mode) == 0o700
def peer():
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(5)
    s.connect(path)
    return s.makefile('rwb', buffering=0)
def call(s, key, n):
    s.write(json.dumps(dict(id=n, binding='lookup', args=dict(values=dict(key=key)))).encode()+b'\n')
pid = os.fork()
if pid == 0:
    try:
        s = peer(); call(s, 'descendant', 1); assert not s.readline()
    except (ConnectionResetError, BrokenPipeError):
        pass
    os._exit(0)
assert os.waitpid(pid, 0)[1] == 0
s = peer(); call(s, 'runner', 1)
assert json.loads(s.readline()) == dict(id=1, value=dict(values=dict(value='bound')))
assert not os.path.exists(path)
print('{"binding":"business data"}')
call(s, 'bad id', 1)
assert not s.readline()
"#]).stdout(Stdio::piped());
    let listener = Listener::prepare(&mut command, &sites, handlers.clone())
        .unwrap()
        .unwrap();
    let child = command.spawn().unwrap();
    let (errors, reported) = mpsc::channel();
    let server = listener
        .start(child.id(), &StreamCancellation::default(), move |err| {
            errors.send(err).unwrap()
        })
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"{\"binding\":\"business data\"}\n");
    assert!(
        reported
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .to_string()
            .contains("out-of-order")
    );
    drop(server);
    assert_eq!(received.try_iter().collect::<Vec<_>>(), ["runner"]);

    let mut command = Command::new("true");
    let listener = Listener::prepare(&mut command, &sites, handlers)
        .unwrap()
        .unwrap();
    let path = grant(&command).socket;
    let mut child = command.spawn().unwrap();
    let server = listener
        .start(child.id(), &Default::default(), |_| {
            panic!("cancelled listener failed")
        })
        .unwrap();
    assert!(child.wait().unwrap().success());
    drop(server);
    assert!(!path.parent().unwrap().exists());
}
