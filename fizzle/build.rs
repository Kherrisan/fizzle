use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::{env, fmt, fs};

use proc_macro2::TokenStream;
use serde::de;
use serde::{Deserialize, Deserializer};

const DEFAULT_CONFIG_PATH: &str = "./Fizzle.toml";
const FIZZLE_CONFIG_ENV: &str = "FIZZLE_CONFIG";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FizzleConfiguration {
    pub io: HashMap<IoEndpoint, IoInputVariant>,
    #[serde(default)]
    pub process: Vec<ProcessConfiguration>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessConfiguration {
    path: String,
    #[serde(default)]
    when: ProcessTiming,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "lowercase")]
pub enum ProcessTiming {
    /// The desired process is spawned as soon as the Fizzle harness is instantiated.
    OnStartup,
    /// The desired process is spawned once the main process has halted (immediately prior to the first fuzzing round).
    OnReady,
}

impl Default for ProcessTiming {
    fn default() -> Self {
        Self::OnStartup
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub enum IoInputVariant {
    Basic(IoBasicMethod),
    Plugin(IoPluginConfiguration),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "lowercase")]
pub enum IoBasicMethod {
    Fuzz,
    Sink,
    Nullsink,
    Passthrough,
    Feedback,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "lowercase")]
pub enum IoTiming {
    /// Initializes the plugin once and maintains state throughout different fuzzing rounds.
    Startup,
    /// Re-initializes the communication channel each time the program is run.
    PerRound,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IoPluginConfiguration {
    pub method: String, // TODO: this must be "plugin"
    pub when: IoTiming,
    pub module: String,
    pub plugin: String,
    pub streams: Option<usize>,
    pub config: Option<toml::Table>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum IoEndpoint {
    File(PathBuf),
    TcpClient(SocketAddr),
    TcpServer(SocketAddr),
    UdpClient(SocketAddr),
    UdpServer(SocketAddr),
    SctpClient(SocketAddr),
    SctpServer(SocketAddr),
    Stdio,
}

impl<'de> Deserialize<'de> for IoEndpoint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(IoEndpointVisitor)
    }
}

struct IoEndpointVisitor;

impl<'de> de::Visitor<'de> for IoEndpointVisitor {
    type Value = IoEndpoint;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a location of form of \"file:<filepath>\", \"<uri>:<addr>\" or \"stdio\"")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value.split_once(':') {
            Some(("file", path)) => {
                Ok(IoEndpoint::File(path.parse().map_err(|_| {
                    de::Error::custom(format!("invalid socket address \"{}\"", path))
                })?))
            }
            Some(("tcp-client", addr)) => {
                Ok(IoEndpoint::TcpClient(addr.parse().map_err(|_| {
                    de::Error::custom(format!("invalid socket address \"{}\"", addr))
                })?))
            }
            Some(("tcp-server", addr)) => {
                Ok(IoEndpoint::TcpServer(addr.parse().map_err(|_| {
                    de::Error::custom(format!("invalid socket address \"{}\"", addr))
                })?))
            }
            Some(("udp-client", addr)) => {
                Ok(IoEndpoint::UdpClient(addr.parse().map_err(|_| {
                    de::Error::custom(format!("invalid socket address \"{}\"", addr))
                })?))
            }
            Some(("udp-server", addr)) => {
                Ok(IoEndpoint::UdpServer(addr.parse().map_err(|_| {
                    de::Error::custom(format!("invalid socket address \"{}\"", addr))
                })?))
            }
            Some(("sctp-client", addr)) => {
                Ok(IoEndpoint::SctpClient(addr.parse().map_err(|_| {
                    de::Error::custom(format!("invalid socket address \"{}\"", addr))
                })?))
            }
            Some(("sctp-server", addr)) => {
                Ok(IoEndpoint::SctpServer(addr.parse().map_err(|_| {
                    de::Error::custom(format!("invalid socket address \"{}\"", addr))
                })?))
            }
            None if value == "stdio" => Ok(IoEndpoint::Stdio),
            _ => Err(de::Error::custom(format!(
                "invalid I/O endpoint \"{}\"",
                value
            ))),
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=Cargo.toml");
    println!("cargo::rerun-if-env-changed={}", FIZZLE_CONFIG_ENV);

    let config_path = match env::var(FIZZLE_CONFIG_ENV) {
        Ok(s) => s,
        Err(_) => DEFAULT_CONFIG_PATH.to_owned(),
    };

    println!("cargo::rerun-if-changed={}", config_path);

    let config_string = fs::read_to_string(config_path)?;
    let config: FizzleConfiguration = toml::from_str(&config_string)?;
    let includes = extract_includes(&config);
    let plugins_impl = gen_populate_plugins(&config);
    let (onstartup_process_impl, onready_process_impl) = gen_processes(&config);

    let final_tokens = quote::quote! {
        #[allow(unused)]
        use fizzle_plugin::IoEndpointVariant;
        #[allow(unused)]
        use fizzle_plugin::{Plugin, PluginObject};
        #[allow(unused)]
        use crate::handlers::plugin::PluginEndpointId;
        #[allow(unused)]
        use crate::handlers::plugin_module::PluginId;
        #[allow(unused)]
        use crate::plugins::{IoEmulationType, Plugins, PluginEndpoint};
        #[allow(unused)]
        use std::path::PathBuf;
        #[allow(unused)]
        use std::net::SocketAddr;
        #[allow(unused)]
        use std::collections::HashMap;

        #includes

        pub fn populate_plugins(endpoints: &mut Vec<PluginEndpoint>, modules: &mut Plugins) {
            #plugins_impl
        }

        pub fn populate_onstartup_processes(#[allow(unused)] processes: &mut Vec<std::process::Command>) {
            #onstartup_process_impl
        }

        pub fn populate_onready_processes(#[allow(unused)] processes: &mut Vec<std::process::Command>) {
            #onready_process_impl
        }
    };
    fs::write("src/comptime.rs", final_tokens.to_string())?;
    Command::new("rustfmt").arg("src/comptime.rs").output()?;

    Ok(())
}

fn extract_includes(config: &FizzleConfiguration) -> TokenStream {
    let mut include_tokens = TokenStream::new();
    let mut includes = HashSet::new();
    for input_variant in config.io.values() {
        if let IoInputVariant::Plugin(plugin_config) = input_variant {
            includes.insert(plugin_config.module.clone());
        }
    }

    for module in includes {
        let module = quote::format_ident!("{}", str::replace(&module, "-", "_"));
        include_tokens.extend(quote::quote! {
            use #module;
        });
    }
    include_tokens
}

fn gen_processes(config: &FizzleConfiguration) -> (TokenStream, TokenStream) {
    #[allow(unused_mut)]
    let mut onstartup_process_tokens = TokenStream::new();
    #[allow(unused_mut)]
    let mut onready_process_tokens = TokenStream::new();

    for process_info in config.process.iter() {
        let tokens = match &process_info.when {
            ProcessTiming::OnStartup => &mut onstartup_process_tokens,
            ProcessTiming::OnReady => {
                unimplemented!() // &mut onready_process_tokens
            }
        };

        let path = &process_info.path;

        tokens.extend(quote::quote! {
            let mut command = std::process::Command::new(#path);
        });

        for arg in &process_info.args {
            tokens.extend(quote::quote! {
                command.arg(#arg);
            });
        }

        tokens.extend(quote::quote! {
            processes.push(command);
        })
    }

    (onstartup_process_tokens, onready_process_tokens)
}

fn gen_populate_plugins(config: &FizzleConfiguration) -> TokenStream {
    let mut modules: HashMap<(String, String), (usize, HashMap<IoEndpoint, toml::Table>)> =
        HashMap::new();
    let mut next_module_id = 0usize;

    let mut populate_plugins_tokens = TokenStream::new();
    let mut comptime_output_tokens = TokenStream::new();

    for (endpoint, input_variant) in config.io.iter() {
        let io_variant = match input_variant {
            IoInputVariant::Basic(IoBasicMethod::Feedback) => quote::quote! {
                let num_streams = 1;
                let emulation_type = IoEmulationType::Feedback;
                let is_per_round = false;
            },
            IoInputVariant::Basic(IoBasicMethod::Fuzz) => quote::quote! {
                let num_streams = 1;
                let emulation_type = IoEmulationType::Fuzz;
                let is_per_round = true;
            },
            IoInputVariant::Basic(IoBasicMethod::Nullsink) => quote::quote! {
                let num_streams = 1;
                let emulation_type = IoEmulationType::NullSink;
                let is_per_round = false;
            },
            IoInputVariant::Basic(IoBasicMethod::Passthrough) => quote::quote! {
                let num_streams = 1;
                let emulation_type = IoEmulationType::Passthrough;
                let is_per_round = false;
            },
            IoInputVariant::Basic(IoBasicMethod::Sink) => quote::quote! {
                let num_streams = 1;
                let emulation_type = IoEmulationType::Sink;
                let is_per_round = false;
            },
            IoInputVariant::Plugin(plugin_config) => {
                let num_streams = plugin_config.streams.unwrap_or(1);
                let module = &plugin_config.module;
                let plugin = &plugin_config.plugin;
                let mod_plug = (module.clone(), plugin.clone());

                let plugin_module_id = if let Some(config) = plugin_config.config.clone() {
                    match modules.entry(mod_plug) {
                        Entry::Occupied(mut o) => {
                            o.get_mut().1.insert(endpoint.clone(), config);
                            o.get().0
                        }
                        Entry::Vacant(v) => {
                            let mut m = HashMap::new();
                            let plugin_module_id = next_module_id;
                            next_module_id += 1;
                            m.insert(endpoint.clone(), config);
                            v.insert((plugin_module_id, m));
                            plugin_module_id
                        }
                    }
                } else {
                    match modules.entry(mod_plug) {
                        Entry::Vacant(v) => {
                            let plugin_module_id = next_module_id;
                            next_module_id += 1;
                            v.insert((plugin_module_id, HashMap::new()));
                            plugin_module_id
                        }
                        Entry::Occupied(o) => o.get().0,
                    }
                };

                let is_per_round = plugin_config.when == IoTiming::PerRound;
                let key_id = quote::format_ident!("key_{}", plugin_module_id);

                quote::quote! {
                    let num_streams = #num_streams;
                    let emulation_type = IoEmulationType::Plugin(#key_id.clone()); // TODO
                    let is_per_round = #is_per_round;
                }
            }
        };

        // Populate plugin endpoints
        populate_plugins_tokens.extend(gen_io_endpoint_def(endpoint));
        populate_plugins_tokens.extend(io_variant);
        populate_plugins_tokens.extend(quote::quote! {
            endpoints.push(PluginEndpoint {
                endpoint_variant,
                emulation_type,
                is_per_round,
                num_streams,
            });
        });
    }

    // Populate (and initialize) plugin modules

    for ((module, plugin), (module_id, endpoint_configs)) in modules {
        comptime_output_tokens.extend(quote::quote! {
            #[allow(unused)]
            let mut endpoint_toml_configs = HashMap::new();
        });

        for (endpoint, table) in endpoint_configs {
            let endpoint_quote = match endpoint {
                IoEndpoint::File(path) => {
                    let path = path.to_str().unwrap();
                    quote::quote! {
                        IoEndpointVariant::File(#path.parse().unwrap())
                    }
                }
                IoEndpoint::TcpClient(addr) => {
                    let addr = addr.to_string();
                    quote::quote! {
                        IoEndpointVariant::TcpClient(#addr.parse().unwrap())
                    }
                }
                IoEndpoint::TcpServer(addr) => {
                    let addr = addr.to_string();
                    quote::quote! {
                        IoEndpointVariant::TcpServer(#addr.parse().unwrap())
                    }
                }
                IoEndpoint::UdpClient(addr) => {
                    let addr = addr.to_string();
                    quote::quote! {
                        IoEndpointVariant::UdpClient(#addr.parse().unwrap())
                    }
                }
                IoEndpoint::UdpServer(addr) => {
                    let addr = addr.to_string();
                    quote::quote! {
                        IoEndpointVariant::UdpServer(#addr.parse().unwrap())
                    }
                }
                IoEndpoint::SctpClient(addr) => {
                    let addr = addr.to_string();
                    quote::quote! {
                        IoEndpointVariant::SctpClient(#addr.parse().unwrap())
                    }
                }
                IoEndpoint::SctpServer(addr) => {
                    let addr = addr.to_string();
                    quote::quote! {
                        IoEndpointVariant::SctpServer(#addr.parse().unwrap())
                    }
                }
                IoEndpoint::Stdio => quote::quote! {
                    IoEndpointVariant::Stdio
                },
            };

            let table_str = table.to_string();
            comptime_output_tokens.extend(quote::quote! {
                endpoint_toml_configs.insert(#endpoint_quote, #table_str.parse::<toml::Table>().unwrap());
            });
        }
        let module = quote::format_ident!("{}", str::replace(&module, "-", "_"));
        let plugin = quote::format_ident!("{}", str::replace(&plugin, "-", "_"));
        let key_id = quote::format_ident!("key_{}", module_id);
        comptime_output_tokens.extend(quote::quote! {
            let #key_id = modules.allocate(Box::new(#module::#plugin::new(endpoint_toml_configs))).unwrap(); // TODO
        });
    }

    comptime_output_tokens.extend(populate_plugins_tokens);
    comptime_output_tokens
}

fn gen_io_endpoint_def(endpoint: &IoEndpoint) -> TokenStream {
    match endpoint {
        IoEndpoint::File(path) => {
            let path = path.to_str().unwrap();
            quote::quote! {
                let endpoint_variant = IoEndpointVariant::File(#path.parse::<PathBuf>().unwrap());
            }
        }
        IoEndpoint::TcpClient(addr) => {
            let addr = addr.to_string();
            quote::quote! {
                let endpoint_variant = IoEndpointVariant::TcpClient(#addr.parse::<SocketAddr>().unwrap());
            }
        }
        IoEndpoint::TcpServer(addr) => {
            let addr = addr.to_string();
            quote::quote! {
                let endpoint_variant = IoEndpointVariant::TcpServer(#addr.parse::<SocketAddr>().unwrap());
            }
        }
        IoEndpoint::UdpClient(addr) => {
            let addr = addr.to_string();
            quote::quote! {
                let endpoint_variant = IoEndpointVariant::UdpClient(#addr.parse::<SocketAddr>().unwrap());
            }
        }
        IoEndpoint::UdpServer(addr) => {
            let addr = addr.to_string();
            quote::quote! {
                let endpoint_variant = IoEndpointVariant::UdpServer(#addr.parse::<SocketAddr>().unwrap());
            }
        }
        IoEndpoint::SctpClient(addr) => {
            let addr = addr.to_string();
            quote::quote! {
                let endpoint_variant = IoEndpointVariant::SctpClient(#addr.parse::<SocketAddr>().unwrap());
            }
        }
        IoEndpoint::SctpServer(addr) => {
            let addr = addr.to_string();
            quote::quote! {
                let endpoint_variant = IoEndpointVariant::SctpServer(#addr.parse::<SocketAddr>().unwrap());
            }
        }
        IoEndpoint::Stdio => quote::quote! {
            let endpoint_variant = IoEndpointVariant::Stdio;
        },
    }
}
