// Copyright (c) The Swiboe development team. All rights reserved.
// Licensed under the Apache License, Version 2.0. See LICENSE.txt
// in the project root for license information.

use ::ipc_bridge;
use ::rpc;
use ::server;
use serde_json;

#[derive(Serialize, Deserialize, Debug)]
pub struct NewRpcRequest {
    pub priority: u16,
    pub name: String,
}

pub struct CorePlugin {
    commands: server::CommandSender,
}

impl CorePlugin {
    pub fn new(commands: server::CommandSender) -> Self {
        CorePlugin {
            commands: commands,
        }
    }

    pub fn call(&self, caller: ipc_bridge::ClientId, rpc_call: rpc::Call) -> Option<rpc::Result> {
        match &*rpc_call.function as &str {
            "core.exit" => {
                self.commands.send(server::Command::Quit).unwrap();
                Some(rpc::Result::success(()))
            },
            "core.new_rpc" => {
                println!("#sirver rpc_call.args: {:#?}", rpc_call.args);
                let args: NewRpcRequest = match serde_json::from_value(rpc_call.args) {
                    Ok(args) => args,
                    Err(err) => return Some(rpc::Result::Err(err.into())),
                };

                println!("#sirver ALIVE {}:{}", file!(), line!());
                self.commands.send(
                    server::Command::NewRpc(caller, rpc_call.context, args.name, args.priority)).unwrap();
                println!("#sirver ALIVE {}:{}", file!(), line!());
                None
            },
            // NOCOM(#sirver): this should not panic, but return an error.
            _ => panic!("{} was called, but is not a core function.", rpc_call.function),
        }
    }
}
