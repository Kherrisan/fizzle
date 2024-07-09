use crate::arena::{ArenaKey, Rc};
use crate::backend::ConnectedBackend;
use crate::state::{FizzleSingleton, PerRoundClientBackend, PerRoundClientInfo};
use crate::{comptime, plugins};

use fizzle_plugin::{IoEndpointVariant, StreamId};
use heapless::FnvIndexSet;

use super::buffer::BufferId;
use super::plugin_module::PluginModuleId;
use super::polled::PolledId;
use super::socket::SocketState;

use std::{mem, thread};

pub use private::PluginId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct PluginId(usize);
}

// Runtime active plugin I/O information
#[derive(Clone, Debug)]
pub struct PluginInfo {
    pub endpoint: IoEndpointVariant,
    pub stream: StreamId,
    /// The plugin module to read/write from.
    pub module_id: Rc<PluginModuleId>,
    pub read_buf: Rc<BufferId>,
    pub read_polled: Rc<PolledId>,
    pub write_buf: Rc<BufferId>,
    pub write_polled: Rc<PolledId>,
}

impl ArenaKey for PluginId {
    type Value = PluginInfo;
}

impl PluginId {

}

pub fn handle_plugins(ctx: &mut FizzleSingleton) {
    let mut state = ctx.acquire();

    if plugins::run_plugins(&mut state) {
        // Plugins have queued more workers as ready
        log::trace!(
            "Plugins emitted new input--yielding thread to start next available worker"
        );

        drop(state);

    } else if !state.global.startup_complete {
        state.global.startup_complete = true;

        // Now run any applicable processes
        let mut processes = Vec::new();
        comptime::populate_onready_processes(&mut processes);

        drop(state);

        for mut process in processes {
            let mut state = ctx.acquire();

            // This thread should still be able to execute afterwards
            state.mark_thread_ready(thread::current().id());

            // TODO: upref all reference-counted global variables here
            // For now we just don't free global variables so it's fine...

            let process_id = state.global.assign_process_id();
            state.global.passthrough_process_id = process_id;

            drop(state);

            process.env("LD_PRELOAD", std::env::var("LD_PRELOAD").unwrap());
            process.spawn().unwrap();

            ctx.pause_current_process();
        }

    } else if !state.global.per_round_endpoints.is_empty() {
        let mut endpoints = FnvIndexSet::new();
        mem::swap(&mut endpoints, &mut state.global.per_round_endpoints);

        for socket_id in endpoints.into_iter() {
            let Some(sock_info) = state.global.sockets.get_mut(&socket_id) else {
                continue
            };

            match sock_info {
                SocketState::PendingConnection(_) => (), // Leave be
                SocketState::Connected(connected) => {
                    log::debug!("removing connected fuzz/plugin client socket");

                    let target_address = connected.local_addr.clone();
                    let source_address = connected.rem_addr.clone();
                    let client_backend = match &connected.backend {
                        ConnectedBackend::Plugin(plugin_id) => PerRoundClientBackend::Plugin(plugin_id.clone()),
                        ConnectedBackend::Fuzz(fuzz_endpoint_id) => PerRoundClientBackend::Fuzz(fuzz_endpoint_id.clone()),
                        _ => unreachable!(),
                    };

                    if !connected.peer_closed {
                        connected.peer_closed = true;

                        // Now raise all applicable poll events so the reader discovers the peer is closed
                        match connected.backend.clone() {
                            ConnectedBackend::Plugin(plugin_id) => {
                                let plugin = state.global.plugins.get(&plugin_id).unwrap();
                                let read_polled = plugin.read_polled.clone();
                                let write_polled = plugin.write_polled.clone();
                                state.raise_polled(&read_polled);
                                state.raise_polled(&write_polled);
                            },
                            ConnectedBackend::Fuzz(fuzz_endpoint_id) => {
                                let read_polled = state.global.fuzz_endpoints.get(&fuzz_endpoint_id).unwrap().read_polled.clone();
                                state.raise_polled(&read_polled);
                            }
                            _ => unreachable!(),
                        }
                    }

                    state.global.per_round_clients.push(PerRoundClientInfo {
                        source_address,
                        target_address,
                        backend: client_backend,
                    }).unwrap();
                }
                _ => unreachable!(),
            }
        }

    } else if !state.global.delayed_ready.is_empty() {
        let ready = state.global.delayed_ready.dequeue().unwrap();
        state.global.ready.enqueue(ready).unwrap();
    
    } else {
        drop(state);

        log::trace!("No workers were ready to execute--fuzzing round complete.");
        // No events were triggered for any pollers--move on to next input
        ctx.fuzz_round_complete();
    }
}