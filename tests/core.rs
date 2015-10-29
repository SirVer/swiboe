// Copyright (c) The Swiboe development team. All rights reserved.
// Licensed under the Apache License, Version 2.0. See LICENSE.txt
// in the project root for license information.

use ::CallbackRpc;
use serde_json;
use std::env;
use std::path;
use std::sync;
use std::thread;
use swiboe::client::RpcCaller;
use swiboe::client;
use swiboe::rpc;
use swiboe::server::Server;
use swiboe::testing::TestHarness;
use uuid::Uuid;

fn temporary_socket_name() -> path::PathBuf {
    let mut dir = env::temp_dir();
    dir.push(format!("{}.socket", Uuid::new_v4().to_string()));
    dir
}

fn as_json(s: &str) -> serde_json::Value {
    serde_json::from_str(s).unwrap()
}

#[test]
fn shutdown_server_with_clients_connected() {
    let socket_name = temporary_socket_name();
    let mut server = Server::launch(&socket_name, &[]).unwrap();

    let _client = client::Client::connect_unix(&socket_name).unwrap();

    server.shutdown();
}

#[test]
fn shutdown_server_with_no_clients_connected() {
    let t = TestHarness::new();
    let _client = client::Client::connect_unix(&t.socket_name).unwrap();
}

struct TestCall {
    priority: u16,
    result: rpc::Result,
}

impl client::rpc::server::Rpc for TestCall {
    fn priority(&self) -> u16 { self.priority }
    fn call(&self, mut context: client::rpc::server::Context, _: serde_json::Value) {
        context.finish(self.result.clone()).unwrap();
    }
}


#[test]
fn new_rpc_simple() {
    let t = TestHarness::new();

    let mut client1 = client::Client::connect_unix(&t.socket_name).unwrap();
    let mut client2 = client::Client::connect_unix(&t.socket_name).unwrap();

    let test_msg = as_json(r#"{ "blub": "blah" }"#);

    client1.new_rpc("test.test", Box::new(TestCall {
        priority: 0,
        result: rpc::Result::Ok(test_msg.clone()),
    })).unwrap();

    let mut rpc = client2.call("test.test", &test_msg).unwrap();
    assert_eq!(rpc.wait().unwrap(), rpc::Result::Ok(test_msg));
}

#[test]
fn new_rpc_invalid_args() {
    let t = TestHarness::new();

    let mut client = client::Client::connect_unix(&t.socket_name).unwrap();
    let args = as_json("{}");
    let result = client.call("core.new_rpc", &args).unwrap().wait().unwrap();
    assert_eq!(rpc::ErrorKind::InvalidArgs, result.unwrap_err().kind);
}

#[test]
fn new_rpc_with_priority() {
    let t = TestHarness::new();

    let mut client1 = client::Client::connect_unix(&t.socket_name).unwrap();
    client1.new_rpc("test.test", Box::new(TestCall {
        priority: 100,
        result: rpc::Result::Ok(as_json(r#"{ "from": "client1" }"#)),
    })).unwrap();


    let mut client2 = client::Client::connect_unix(&t.socket_name).unwrap();
    client2.new_rpc("test.test", Box::new(TestCall {
        priority: 50,
        result: rpc::Result::Ok(as_json(r#"{ "from": "client2" }"#)),
    })).unwrap();

    let mut client3 = client::Client::connect_unix(&t.socket_name).unwrap();
    let mut rpc = client3.call("test.test", &as_json(r#"{}"#)).unwrap();
    assert_eq!(rpc::Result::Ok(as_json(r#"{ "from": "client2" }"#)), rpc.wait().unwrap());
}

#[test]
fn new_rpc_with_priority_first_does_not_handle() {
    let t = TestHarness::new();

    let mut client1 = client::Client::connect_unix(&t.socket_name).unwrap();
    client1.new_rpc("test.test", Box::new(TestCall {
        priority: 100,
        result: rpc::Result::Ok(as_json(r#"{ "from": "client1" }"#)),
    })).unwrap();


    let mut client2 = client::Client::connect_unix(&t.socket_name).unwrap();
    client2.new_rpc("test.test", Box::new(TestCall {
        priority: 50,
        result: rpc::Result::NotHandled,
    })).unwrap();

    let mut client3 = client::Client::connect_unix(&t.socket_name).unwrap();
    let mut rpc = client3.call("test.test", &as_json(r#"{}"#)).unwrap();
    assert_eq!(rpc::Result::Ok(as_json(r#"{ "from": "client1" }"#)), rpc.wait().unwrap());
}

#[test]
fn client_disconnects_should_not_stop_handling_of_rpcs() {
    let t = TestHarness::new();

    let mut client0 = client::Client::connect_unix(&t.socket_name).unwrap();
    client0.new_rpc("test.test", Box::new(TestCall {
            priority: 100, result: rpc::Result::NotHandled,
    })).unwrap();

    let mut client1 = client::Client::connect_unix(&t.socket_name).unwrap();
    client1.new_rpc("test.test", Box::new(TestCall {
            priority: 101, result:
                rpc::Result::Ok(as_json(r#"{ "from": "client1" }"#)),
    })).unwrap();

    let mut client2 = client::Client::connect_unix(&t.socket_name).unwrap();
    client2.new_rpc("test.test", Box::new(TestCall {
            priority: 102, result: rpc::Result::NotHandled,
    })).unwrap();

    let mut client3 = client::Client::connect_unix(&t.socket_name).unwrap();
    client3.new_rpc("test.test", Box::new(TestCall {
            priority: 103, result:
                rpc::Result::Ok(as_json(r#"{ "from": "client3" }"#)),
    })).unwrap();

    let mut client = client::Client::connect_unix(&t.socket_name).unwrap();

    let mut rpc = client.call("test.test", &as_json(r#"{}"#)).unwrap();
    assert_eq!(rpc::Result::Ok(as_json(r#"{ "from": "client1" }"#)), rpc.wait().unwrap());

    drop(client1); // clients: 0 2 3
    let mut rpc = client.call("test.test", &as_json(r#"{}"#)).unwrap();
    assert_eq!(rpc::Result::Ok(as_json(r#"{ "from": "client3" }"#)), rpc.wait().unwrap());

    drop(client0); // clients: 2 3
    let mut rpc = client.call("test.test", &as_json(r#"{}"#)).unwrap();
    assert_eq!(rpc::Result::Ok(as_json(r#"{ "from": "client3" }"#)), rpc.wait().unwrap());

    drop(client3); // clients: 2
    let mut rpc = client.call("test.test", &as_json(r#"{}"#)).unwrap();
    assert_eq!(rpc::Result::NotHandled, rpc.wait().unwrap());

    drop(client2); // clients:

    let mut rpc = client.call("test.test", &as_json(r#"{}"#)).unwrap();
    assert_eq!(rpc::Result::Err(rpc::Error {
        kind: rpc::ErrorKind::UnknownRpc,
        details: None,
    }), rpc.wait().unwrap());
}

#[test]
fn call_not_existing_rpc() {
    let t = TestHarness::new();

    let mut client = client::Client::connect_unix(&t.socket_name).unwrap();
    let mut rpc = client.call("not_existing", &as_json("{}")).unwrap();
    assert_eq!(rpc::Result::Err(rpc::Error {
        kind: rpc::ErrorKind::UnknownRpc,
        details: None,
    }), rpc.wait().unwrap());
}

#[test]
fn call_streaming_rpc_simple() {
    // NOCOM(#sirver): test for next_result on non streaming rpc
    let t = TestHarness::new();

    let mut streaming_client = client::Client::connect_unix(&t.socket_name).unwrap();
    streaming_client.new_rpc("test.test", Box::new(CallbackRpc {
        priority: 50,
        callback: |mut context: client::rpc::server::Context, _| {
            thread::spawn(move || {
                context.update(&as_json(r#"{ "msg": "one" }"#)).unwrap();
                context.update(&as_json(r#"{ "msg": "two" }"#)).unwrap();
                context.update(&as_json(r#"{ "msg": "three" }"#)).unwrap();
                context.finish(rpc::Result::success(&as_json(r#"{ "foo": "blah" }"#))).unwrap();
            });
        },
    })).unwrap();

    let mut client = client::Client::connect_unix(&t.socket_name).unwrap();
    let mut rpc = client.call("test.test", &as_json("{}")).unwrap();

    assert_eq!(as_json(r#"{ "msg": "one" }"#), rpc.recv().unwrap().unwrap());
    assert_eq!(as_json(r#"{ "msg": "two" }"#), rpc.recv().unwrap().unwrap());
    assert_eq!(as_json(r#"{ "msg": "three" }"#), rpc.recv().unwrap().unwrap());
    assert_eq!(rpc::Result::success(as_json(r#"{ "foo": "blah" }"#)), rpc.wait().unwrap());
}

#[test]
fn call_streaming_rpc_cancelled() {
    let cancelled = sync::Arc::new(sync::Mutex::new(false));

    let t = TestHarness::new();
    let mut streaming_client = client::Client::connect_unix(&t.socket_name).unwrap();
    let cancelled_clone = cancelled.clone();
    streaming_client.new_rpc("test.test", Box::new(CallbackRpc {
        priority: 50,
        callback: move |mut context: client::rpc::server::Context, _| {
            let cancelled = cancelled_clone.clone();
            thread::spawn(move || {
                let mut count = 0;
                // NOCOM(#sirver): cancelled? grep for that.
                while !context.cancelled() {
                    // It might have been cancelled between our check and now, so ignore errors.
                    let _ = context.update(
                        &as_json(&format!(r#"{{ "value": "{}" }}"#, count)));
                    thread::sleep_ms(10);
                    count += 1
                }
                assert!(context.finish(rpc::Result::success(
                            &as_json(r#"{ "foo": "blah" }"#))).is_err());
                let mut cancelled = cancelled.lock().unwrap();
                *cancelled = true;
            });
        },
    })).unwrap();

    let mut client = client::Client::connect_unix(&t.socket_name).unwrap();
    let mut rpc = client.call("test.test", &as_json("{}")).unwrap();

    assert_eq!(as_json(r#"{ "value": "0" }"#), rpc.recv().unwrap().unwrap());
    assert_eq!(as_json(r#"{ "value": "1" }"#), rpc.recv().unwrap().unwrap());
    assert_eq!(as_json(r#"{ "value": "2" }"#), rpc.recv().unwrap().unwrap());
    assert_eq!(as_json(r#"{ "value": "3" }"#), rpc.recv().unwrap().unwrap());

    rpc.cancel().unwrap();

    // Wait for the server thread to end. If anything went wrong this will sit forever.
    loop {
        let cancelled = cancelled.lock().unwrap();
        if *cancelled == true {
            break;
        }
    }
}
