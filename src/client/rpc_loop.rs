// Copyright (c) The Swiboe development team. All rights reserved.
// Licensed under the Apache License, Version 2.0. See LICENSE.txt
// in the project root for license information.

use ::client::rpc;
use ::error::Result;
use ::ipc;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use threadpool::ThreadPool;


pub type CommandSender = mpsc::Sender<Command>;
pub enum Command {
    Quit,
    NewRpc(String, Box<rpc::server::Rpc>),
    Received(::ipc::Message),
    OutgoingCall(String, mpsc::Sender<::rpc::Response>, ipc::Message),
    CancelOutgoingRpc(String),
    Send(::ipc::Message),
}

struct RunningRpc {
    commands: mpsc::Sender<rpc::server::Command>,
}

impl RunningRpc {
    fn new(commands: mpsc::Sender<rpc::server::Command>) -> Self {
        RunningRpc {
            commands: commands,
        }
    }
}

// NOCOM(#sirver): name is no longer fitting
struct RpcLoop {
    remote_procedures: HashMap<String, Arc<Box<rpc::server::Rpc>>>,
    send_queue: mpsc::Sender<ipc::Message>,
    running_rpc_calls: HashMap<String, RunningRpc>,
    command_sender: CommandSender,
    // NOCOM(#sirver): maybe not use a channel to send data to rpcs?
    running_function_calls: HashMap<String, mpsc::Sender<::rpc::Response>>,
    thread_pool: ThreadPool,
}

impl RpcLoop {
    fn spin_forever(&mut self, commands: mpsc::Receiver<Command>) -> Result<()> {
        while let Ok(command) = commands.recv() {
            match command {
                Command::Quit => break,
                Command::NewRpc(name, rpc) => {
                    self.remote_procedures.insert(name, Arc::new(rpc));
                },
                Command::Received(message) => {
                    match message {
                        ::ipc::Message::RpcCall(rpc_call) => {
                            println!("#sirver rpc_call.function: {:#?}", rpc_call.function);
                            println!("#sirver self.remote_procedures.len(): {:#?}", self.remote_procedures.len());
                            if let Some(function) = self.remote_procedures.get(&rpc_call.function) {
                                let (tx, rx) = mpsc::channel();
                                self.running_rpc_calls.insert(rpc_call.context.clone(), RunningRpc::new(tx));
                                let command_sender = self.command_sender.clone();
                                let function = function.clone();
                                self.thread_pool.execute(move || {
                                    function.call(rpc::server::Context::new(
                                            rpc_call.context, rx, command_sender), rpc_call.args);
                                })
                            }
                            // NOCOM(#sirver): return an error - though if that has happened the
                            // server messed up too.
                        },
                        ::ipc::Message::RpcCancel(rpc_cancel) => {
                            // NOCOM(#sirver): on drop, the rpcservercontext must delete the entry.
                            if let Some(function) = self.running_rpc_calls.remove(&rpc_cancel.context) {
                                // The function might be dead already, so we ignore errors.
                                let _ = function.commands.send(rpc::server::Command::Cancel);
                            }
                        },
                        ipc::Message::RpcResponse(rpc_data) => {
                            // NOCOM(#sirver): if this is a streaming RPC, we should cancel the
                            // RPC.
                            // This will quietly drop any updates on functions that we no longer
                            // know/care about.
                            self.running_function_calls
                                .get(&rpc_data.context)
                                .map(|channel| {
                                    // The other side of this channel might not exist anymore - we
                                    // might have dropped the RPC already. Just ignore it.
                                    let _ = channel.send(rpc_data);
                                });
                        },
                    }
                },
                Command::Send(message) => {
                    try!(self.send_queue.send(message));
                },
                Command::OutgoingCall(context, tx, message) => {
                    self.running_function_calls.insert(context, tx);
                    // NOCOM(#sirver): can the message be constructed here?
                    try!(self.send_queue.send(message));
                }
                Command::CancelOutgoingRpc(context) => {
                    let msg = ::ipc::Message::RpcCancel(::rpc::Cancel {
                        context: context,
                    });
                    try!(self.send_queue.send(msg));
                }
            }
        };
        Ok(())
    }
}

pub fn spawn<'a>(commands: mpsc::Receiver<Command>,
                 command_sender: CommandSender,
                 send_queue: mpsc::Sender<ipc::Message>) -> thread::JoinHandle<()>
{
    thread::spawn(move || {
        let mut thread = RpcLoop {
            remote_procedures: HashMap::new(),
            running_function_calls: HashMap::new(),
            running_rpc_calls: HashMap::new(),
            send_queue: send_queue,
            command_sender: command_sender,
            // NOCOM(#sirver): that seems silly.
            thread_pool: ThreadPool::new(1),
        };
        match thread.spin_forever(commands) {
            Ok(_) => (),
            Err(err) => println!("RpcLoop::spin_forever returned an error: {}", err),
        }
    })
}
