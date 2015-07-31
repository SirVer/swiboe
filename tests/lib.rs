extern crate serde;
extern crate switchboard;
extern crate uuid;

use serde::json;
use std::env;
use std::path::{PathBuf};
use switchboard::client::Client;
use switchboard::server::{Server, RpcResultKind};
use uuid::Uuid;

// NOCOM(#sirver): use the name switchboard everywhere.

fn temporary_socket_name() -> PathBuf {
    let mut dir = env::temp_dir();
    dir.push(format!("{}.socket", Uuid::new_v4().to_string()));
    dir
}

#[test]
fn start_and_kill_server() {
    let mut s = Server::launch(&temporary_socket_name());
    s.shutdown();
}

#[test]
fn broadcast_works() {
    let socket_name = temporary_socket_name();
    let mut s = Server::launch(&socket_name);

    let mut client1 = Client::connect(&socket_name.to_string_lossy());
    let mut client2 = Client::connect(&socket_name.to_string_lossy());

    let test_msg = json::builder::ObjectBuilder::new()
        .insert("blub".into(), "blah")
        .unwrap();

    let function_call = client1.call("core.broadcast", &test_msg);
    assert_eq!(function_call.wait(), RpcResultKind::Ok);

    let broadcast_msg = client1.recv();
    assert_eq!(test_msg, broadcast_msg);

    let broadcast_msg = client2.recv();
    assert_eq!(test_msg, broadcast_msg);

    s.shutdown();
}
