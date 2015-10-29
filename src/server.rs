// Copyright (c) The Swiboe development team. All rights reserved.
// Licensed under the Apache License, Version 2.0. See LICENSE.txt
// in the project root for license information.

use ::error::{Error, Result};
use ::ipc;
use ::ipc_bridge;
use ::plugin_buffer;
use ::plugin_core;
use ::plugin_list_files;
use ::rpc;
use mio;
use serde_json;
use std::collections::Bound::{Included, Unbounded};
use std::collections::btree_map;
use std::collections::hash_map;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread;

// NOCOM(#sirver): when a client disconnects and we still try to call one of it's rpcs, we never
// get an error back - this will effectively interrupt the rpc call stack.
// NOCOM(#sirver): document everything.
const CORE_FUNCTIONS_PREFIX: &'static str = "core.";

pub enum Command {
    Quit,
    NewRpc(ipc_bridge::ClientId, String, String, u16),
    RpcCall(ipc_bridge::ClientId, rpc::Call),
    RpcResponse(rpc::Response),
    RpcCancel(rpc::Cancel),
    ClientConnected(ipc_bridge::ClientId),
    ClientDisconnected(ipc_bridge::ClientId),
    SendDataFailed(ipc_bridge::ClientId, ipc::Message, Error),
}
pub type CommandSender = Sender<Command>;

#[derive(Debug)]
struct RegisteredFunction {
    client_id: ipc_bridge::ClientId,
    priority: u16,
}

#[derive(Debug)]
struct RunningRpc {
    caller: ipc_bridge::ClientId,
    args: serde_json::Value,
    last_index: isize,
    matching_rpc_names: Vec<String>,
}

pub struct Swiboe {
    functions: BTreeMap<String, RegisteredFunction>,
    commands: Receiver<Command>,
    clients: HashSet<ipc_bridge::ClientId>,
    ipc_bridge_commands: mio::Sender<ipc_bridge::Command>,
    running_rpcs: HashMap<String, RunningRpc>,
    plugin_core: plugin_core::CorePlugin,
}

impl Swiboe {
    pub fn spin_forever(&mut self) -> Result<()> {
        while let Ok(command) = self.commands.recv() {
            match command {
                Command::Quit => break,
                Command::NewRpc(client_id, rpc_call_context, name, priority) => {
                    // NOCOM(#sirver): deny everything starting with 'core'
                    // NOCOM(#sirver): make sure the client_id is known.
                    // NOCOM(#sirver): make sure the client has not already registered this
                    // function.
                    let result = match self.functions.entry(name) {
                        btree_map::Entry::Vacant(entry) => {
                            let vec = entry.insert(
                                RegisteredFunction {
                                    client_id: client_id,
                                    priority: priority,
                                });
                            rpc::Result::success(())
                        },
                        btree_map::Entry::Occupied(_) => {
                            rpc::Result::Err(
                                rpc::Error {
                                    kind: rpc::ErrorKind::InvalidArgs,
                                    details: serde_json::from_str("\"rpc_name_already_used\"").unwrap(),
                                })
                        },
                    };
                    try!(self.ipc_bridge_commands.send(ipc_bridge::Command::SendData(
                                client_id,
                                ipc::Message::RpcResponse(rpc::Response {
                                    context: rpc_call_context,
                                    kind: rpc::ResponseKind::Last(result),
                                }))));
                },
                Command::RpcCall(client_id, rpc_call) => {
                    // NOCOM(#sirver): make sure this is not already in running_rpcs.
                    // NOCOM(#sirver): function name might not be in there.

                    // Special case 'core.'. We handle them immediately.
                    if rpc_call.function.starts_with(CORE_FUNCTIONS_PREFIX) {
                        let context = rpc_call.context.clone();
                        let result = self.plugin_core.call(client_id, rpc_call);
                        // NOCOM(#sirver): that feels just plain silly. why even return a result?
                        if let Some(result) = result {
                            try!(self.ipc_bridge_commands.send(ipc_bridge::Command::SendData(
                                        client_id,
                                        ipc::Message::RpcResponse(rpc::Response {
                                            context: context,
                                            kind: rpc::ResponseKind::Last(result),
                                        }))));
                        }
                    } else {
                        let matching_rpc_names: Vec<String> = {
                           let mut matching_rpcs = Vec::new();
                            let it: btree_map::Range<String, RegisteredFunction> =
                                self.functions.range(Included(&rpc_call.function), Unbounded::<&str>);
                            // NOCOM(#sirver): drop type and inline?
                            for (name, function) in it  {
                                if !name.starts_with(&rpc_call.function) {
                                    break;
                                }
                                matching_rpcs.push((function.priority, name));
                            }
                            // NOCOM(#sirver): is this sorting correct?
                            matching_rpcs.sort_by(|a, b| { a.0.cmp(&b.0) });
                            matching_rpcs.iter().map(|e| e.1.to_string()).collect()
                        };

                        if matching_rpc_names.is_empty() {
                            try!(self.ipc_bridge_commands.send(ipc_bridge::Command::SendData(
                                        client_id,
                                        ipc::Message::RpcResponse(rpc::Response {
                                            context: rpc_call.context.clone(),
                                            kind: rpc::ResponseKind::Last(rpc::Result::Err(rpc::Error {
                                                kind: rpc::ErrorKind::UnknownRpc,
                                                details: None,
                                            })),
                                        }))));

                        } else {
                            let mut running_rpc = RunningRpc {
                                caller: client_id.clone(),
                                args: rpc_call.args,
                                last_index: -1,
                                matching_rpc_names: matching_rpc_names,
                            };
                            self.running_rpcs.insert(rpc_call.context.clone(), running_rpc);
                            self.call_next_implementor(&rpc_call.context);
                        }
                    }
                },
                Command::RpcResponse(rpc_response) => {
                    try!(self.on_rpc_response(rpc_response));
                },
                Command::RpcCancel(rpc_cancel) => {
                    try!(self.on_rpc_cancel(rpc_cancel));
                },
                Command::SendDataFailed(client_id, msg, err) => {
                    let action = match msg {
                        ipc::Message::RpcResponse(_) | ipc::Message::RpcCancel(_) => {
                            // NOCOM(#sirver): on a streaming rpc, this should also try to cancel
                            // the RPC.
                            "dropped the RpcResponse/RpcCall."
                        },
                        ipc::Message::RpcCall(rpc_call) => {
                            try!(self.on_rpc_response(rpc::Response {
                                context: rpc_call.context,
                                kind: rpc::ResponseKind::Last(rpc::Result::NotHandled),
                            }));
                            "surrogate replied as NotHandled."
                        }
                    };
                    println!("Sending to {:?} failed: {:?}, {}", client_id, err, action);
                },
                Command::ClientConnected(client_id) => {
                    // NOCOM(#sirver): make sure client_id is not yet known.
                    self.clients.insert(client_id);
                },
                Command::ClientDisconnected(client_id) => {
                    self.clients.remove(&client_id);

                    // Kill all pending RPCs that have been requested by this client.
                    let rpcs_to_remove: Vec<_> = self.running_rpcs.iter()
                        .filter_map(|(context, running_rpc)| {
                            if running_rpc.caller == client_id {
                                Some(context.to_string())
                            } else {
                                None
                            }
                        })
                        .collect();
                    for context in rpcs_to_remove {
                        self.running_rpcs.remove(&context);
                    }

                    // Kill all functions that have been registered by this.
                    // NOCOM(#sirver): figure that out again.
                    // let mut functions_to_remove = Vec::new();
                    // for (function_name, registered_functions) in &mut self.functions {
                        // registered_functions.retain(|registered_function| {
                            // registered_function.client_id != client_id
                        // });
                        // if registered_functions.is_empty() {
                            // functions_to_remove.push(function_name.to_string());
                        // }
                    // }
                    // for function_name in functions_to_remove {
                        // self.functions.remove(&function_name);
                    // }
                }
            }
        };
        Ok(())
    }

    fn on_rpc_cancel(&mut self, rpc_cancel: rpc::Cancel) -> Result<()> {
        unimplemented!();
        // let running_rpc = match self.running_rpcs.entry(rpc_cancel.context.clone()) {
            // hash_map::Entry::Occupied(running_rpc) => running_rpc,
            // hash_map::Entry::Vacant(_) => {
                // // Unknown RPC. We simply drop this message.
                // return Ok(());
            // }
        // };

        // // NOCOM(#sirver): only the original caller can cancel, really.
        // let running_rpc = running_rpc.remove();
        // match {
            // // NOCOM(#sirver): quite some code duplication with RpcCall
            // self.functions.get(&running_rpc.rpc_call.function as &str).and_then(|vec| {
                // vec.get(running_rpc.last_index)
            // })
        // } {
            // Some(function) => {
                // // NOCOM(#sirver): eventually, when we keep proper track of our rpc calls, this should be
                // // able to move again.
                // try!(self.ipc_bridge_commands.send(ipc_bridge::Command::SendData(
                        // function.client_id,
                        // ipc::Message::RpcCancel(rpc_cancel)
                        // )));
            // },
            // None => {
                // // NOCOM(#sirver): Wait what... nothing to cancel?
            // }
        // };
        // Ok(())
    }

    fn on_rpc_response(&mut self, rpc_response: rpc::Response) -> Result<()> {
        // NOCOM(#sirver): use get() instead.
        let mut running_rpc = match self.running_rpcs.entry(rpc_response.context.clone()) {
            hash_map::Entry::Occupied(running_rpc) => running_rpc,
            hash_map::Entry::Vacant(_) => {
                // Unknown RPC. We simply drop this message.
                return Ok(());
            }
        };

        match rpc_response.kind {
            rpc::ResponseKind::Partial(value) => {
                let running_rpc = running_rpc.get();
                try!(self.ipc_bridge_commands.send(ipc_bridge::Command::SendData(
                        running_rpc.caller,
                        ipc::Message::RpcResponse(rpc::Response {
                            context: rpc_response.context,
                            kind: rpc::ResponseKind::Partial(value),
                        }))));
            },
            rpc::ResponseKind::Last(result) => match result {
                rpc::Result::Ok(_) | rpc::Result::Err(_) => {
                    let running_rpc = running_rpc.remove();
                    try!(self.ipc_bridge_commands.send(ipc_bridge::Command::SendData(
                            running_rpc.caller,
                            ipc::Message::RpcResponse(rpc::Response {
                                context: rpc_response.context,
                                kind: rpc::ResponseKind::Last(
                                    result
                                ),
                            }))));
                },
                rpc::Result::NotHandled => {
                    unimplemented!();
                    // // TODO(sirver): If a new function has been registered or been deleted since we
                    // // last saw this context, this might skip a handler or call one twice. We need
                    // // a better way to keep track where we are in the list of handlers.
                    // let running_rpc = running_rpc.get_mut();


                    // running_rpc.last_index += 1;
                    // match {
                        // // NOCOM(#sirver): quite some code duplication with RpcCall
                        // self.functions.get(&running_rpc.rpc_call.function as &str).and_then(|vec| {
                            // vec.get(running_rpc.last_index)
                        // })
                    // } {
                        // Some(function) => {
                            // // NOCOM(#sirver): eventually, when we keep proper track of our rpc calls, this should be
                            // // able to move again.
                            // try!(self.ipc_bridge_commands.send(ipc_bridge::Command::SendData(
                                    // function.client_id,
                                    // ipc::Message::RpcCall(running_rpc.rpc_call.clone())
                                    // )));
                        // },
                        // None => {
                            // try!(self.ipc_bridge_commands.send(ipc_bridge::Command::SendData(
                                    // running_rpc.caller,
                                    // ipc::Message::RpcResponse(rpc::Response {
                                        // context: running_rpc.rpc_call.context.clone(),
                                        // kind: rpc::ResponseKind::Last(rpc::Result::NotHandled),
                                    // }))));
                        // }
                    // };
                    // NOCOM(#sirver): we ignore timeouts.
                }
            },
        };
        Ok(())
    }


    fn call_next_implementor(&mut self, context: &str) -> Result<()> {
        let mut running_rpc = self.running_rpcs.get_mut(context)
            .expect("call_next_implementor: Expected the RPC to still run.");

        while running_rpc.last_index < running_rpc.matching_rpc_names.len() as isize {
            running_rpc.last_index += 1;
            let function = &running_rpc.matching_rpc_names[running_rpc.last_index as usize];
            if let Some(registered_function) = self.functions.get(function) {
                let client_rpc_call = rpc::Call {
                    function: function.to_string(),
                    context: context.to_string(),
                    args: running_rpc.args.clone(),
                };

                try!(self.ipc_bridge_commands.send(ipc_bridge::Command::SendData(
                            registered_function.client_id,
                            ipc::Message::RpcCall(client_rpc_call)
                            )));
                return Ok(());
            }
        }

        // NOCOM(#sirver): This is not okay - there was no next implementor. This RPC is done.
        Ok(())
        // NOCOM(#sirver): we ignore timeouts.
    }

}

pub struct Server {
    unix_domain_socket_name: PathBuf,
    tcp_addresses: Vec<String>,
    commands: CommandSender,
    ipc_bridge_commands: mio::Sender<ipc_bridge::Command>,
    swiboe_thread: Option<thread::JoinHandle<()>>,
    event_loop_thread: Option<thread::JoinHandle<()>>,
    buffer_plugin: Option<plugin_buffer::BufferPlugin>,
    list_files_plugin: Option<plugin_list_files::ListFilesPlugin>,
}

impl Server {
    pub fn launch(unix_domain_socket_name: &Path, tcp_addresses: &[&str]) -> Result<Self> {
        let (tx, rx) = channel();

        let mut event_loop = mio::EventLoop::new().expect("Could not create an event loop.");

        let mut server = Server {
            unix_domain_socket_name: unix_domain_socket_name.to_path_buf(),
            tcp_addresses: tcp_addresses.iter().map(|slice| slice.to_string()).collect(),
            commands: tx,
            ipc_bridge_commands: event_loop.channel(),
            buffer_plugin: None,
            list_files_plugin: None,
            swiboe_thread: None,
            event_loop_thread: None,
        };

        let mut swiboe = Swiboe {
            functions: BTreeMap::new(),
            clients: HashSet::new(),
            running_rpcs: HashMap::new(),
            commands: rx,
            ipc_bridge_commands: event_loop.channel(),
            plugin_core: plugin_core::CorePlugin::new(server.commands.clone()),
        };

        let mut ipc_bridge = ipc_bridge::IpcBridge::new(
            &mut event_loop, &server.unix_domain_socket_name, &server.tcp_addresses,
            server.commands.clone());

        server.event_loop_thread = Some(thread::spawn(move || {
            event_loop.run(&mut ipc_bridge).expect("Could not start event_loop.");
        }));

        server.swiboe_thread = Some(thread::spawn(move || {
            loop {
                match swiboe.spin_forever() {
                    Err(err) => println!("#sirver spin_forever: {:#?}", err),
                    Ok(()) => break,
                };
            }
        }));

        println!("#sirver ALIVE {}:{}", file!(), line!());
        server.buffer_plugin = Some(
            try!(plugin_buffer::BufferPlugin::new(&server.unix_domain_socket_name)));
        println!("#sirver ALIVE {}:{}", file!(), line!());
        server.list_files_plugin = Some(
            try!(plugin_list_files::ListFilesPlugin::new(&server.unix_domain_socket_name)));
        println!("#sirver ALIVE {}:{}", file!(), line!());
        Ok(server)
    }

    pub fn shutdown(&mut self) {
        // Any of the threads might have already panicked. So we ignore send errors.
        let _ = self.ipc_bridge_commands.send(ipc_bridge::Command::Quit);
        self.wait_for_event_loop_thread_to_shut_down();
        let _ = self.commands.send(Command::Quit);
        self.wait_for_swiboe_thread_to_shut_down();

        self.wait_for_shutdown();
    }

    fn wait_for_event_loop_thread_to_shut_down(&mut self) {
        if let Some(thread) = self.event_loop_thread.take() {
            thread.join().expect("Could not join event_loop_thread.");
        }
    }

    fn wait_for_swiboe_thread_to_shut_down(&mut self) {
        if let Some(thread) = self.swiboe_thread.take() {
            thread.join().expect("Could not join swiboe_thread.");
        }
    }

    pub fn wait_for_shutdown(&mut self) {
        self.wait_for_event_loop_thread_to_shut_down();
        self.wait_for_swiboe_thread_to_shut_down();

        fs::remove_file(&self.unix_domain_socket_name).expect(
            &format!("Could not remove socket {:?}", self.unix_domain_socket_name));
    }
}
