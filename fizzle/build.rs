use std::net::SocketAddr;
use std::path::PathBuf;
use std::{env, fmt, fs};
use std::collections::HashMap;

use proc_macro2::TokenStream;
use serde::{Deserialize, Deserializer};
use serde::de;

const DEFAULT_CONFIG_PATH: &str = "./Fizzle.toml";
const FIZZLE_CONFIG_ENV: &str = "FIZZLE_CONFIG";

#[derive(Deserialize)]
pub struct FizzleConfiguration {
    pub io: HashMap<String, IoInputVariant>
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum IoInputVariant {
    Basic(IoBasicMethod),
    Plugin(IoPluginConfiguration),
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IoBasicMethod {
    Fuzz,
    Sink,
    Nullsink,
    Passthrough,
    Feedback,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum IoPluginMethod {
    #[serde(rename = "lowercase")]
    Plugin,
}

#[derive(Deserialize)]
pub struct IoPluginConfiguration {
    pub method: IoPluginMethod,
    pub module: String,
    pub plugin: String,
    pub streams: Option<usize>,
    pub config: Option<toml::Table>,
}

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
        D: Deserializer<'de> {
        deserializer.deserialize_str(IoEndpointVisitor)
    }
}

struct IoEndpointVisitor;

impl<'de> de::Visitor<'de> for IoEndpointVisitor {
    type Value = IoEndpoint;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an integer between -2^31 and 2^31")
    }

    fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value.split_once('.') {
            Some(("tcp-client", addr)) => Ok(IoEndpoint::TcpClient(addr.parse().map_err(|_| de::Error::custom(format!("invalid socket address \"{}\"", addr)))?)),
            Some(("tcp-server", addr)) => Ok(IoEndpoint::TcpServer(addr.parse().map_err(|_| de::Error::custom(format!("invalid socket address \"{}\"", addr)))?)),
            Some(("udp-client", addr)) => Ok(IoEndpoint::UdpClient(addr.parse().map_err(|_| de::Error::custom(format!("invalid socket address \"{}\"", addr)))?)),
            Some(("udp-server", addr)) => Ok(IoEndpoint::UdpServer(addr.parse().map_err(|_| de::Error::custom(format!("invalid socket address \"{}\"", addr)))?)),
            Some(("sctp-client", addr)) => Ok(IoEndpoint::SctpClient(addr.parse().map_err(|_| de::Error::custom(format!("invalid socket address \"{}\"", addr)))?)),
            Some(("sctp-server", addr)) => Ok(IoEndpoint::SctpServer(addr.parse().map_err(|_| de::Error::custom(format!("invalid socket address \"{}\"", addr)))?)),
            None if value == "stdio" => Ok(IoEndpoint::Stdio),
            _ => Err(de::Error::custom(format!("invalid I/O endpoint \"{}\"", value))),
        }
    }
}

fn main() {
    let config_path = match env::var(FIZZLE_CONFIG_ENV) {
        Ok(s) => s,
        Err(_) => DEFAULT_CONFIG_PATH.to_owned(),
    };

    let config_string = fs::read_to_string(config_path).unwrap();
    let config: FizzleConfiguration = toml::from_str(&config_string).unwrap();
    let includes = extract_includes(&config);
    let plugins_impl = gen_populate_plugins(&config);
    

    let final_tokens = quote::quote! {
        use super::plugins::PluginConfig;
        #includes

        pub fn populate_plugins(config: &mut PluginConfig) {
            #plugins_impl
        }
    };
    fs::write("src/state/comptime.rs", final_tokens.to_string()).unwrap();
}

fn extract_includes(config: &FizzleConfiguration) -> TokenStream {
    let mut include_tokens = TokenStream::new();
    for input_variant in config.io.values() {
        if let IoInputVariant::Plugin(plugin_config) = input_variant {
            let module = &plugin_config.module;
            include_tokens.extend(quote::quote!{
                use #module;
            });
        }
    }
    include_tokens
}

fn gen_populate_plugins(config: &FizzleConfiguration) -> TokenStream {
    let mut populate_plugins_tokens = TokenStream::new();

    

    populate_plugins_tokens
}

/*
fn decode() {
    // TODO: pull this all out into its own function
    let config_file = "./Fizzle.toml";
    let config_info = fs::read_to_string(config_file).unwrap();
    let config_table = config_info.parse::<toml::Table>().unwrap();

    for (top_level_key, top_level_table) in config_table {
        match top_level_key.as_str() {
            "io" => {
                let toml::Value::Table(io_table) = top_level_table else {
                    panic!("expected \"io\" to be a table");
                };
                for (io_key, io_value) in io_table {
                    match io_value {
                        toml::Value::String(s) => match s.as_str() {
                            "sink" => (),
                            "nullsink" => (),
                            "feedback" => (),
                            "fuzz" => (),
                            "passthrough" => (),
                            "plugin" => panic!("\"plugin\" I/O method must be accompanied by \"module\" and \"plugin\" values"),
                            _ => panic!("unrecognized value \"{}\" for key \"io.{}\"", s, io_key)
                        },
                        toml::Value::Table(io_table) => {
                            let mut method = None;
                            let mut module = None;
                            let mut plugin = None;
                            let mut config = None;
                            for (key, value) in io_table {
                                match key.as_str() {
                                    "method" => {
                                        let toml::Value::String(s) = value else {
                                            panic!("expected string for I/O \"method\" value");
                                        };
                                        method = Some(s.clone());
                                    }
                                    "module" => {
                                        let toml::Value::String(s) = value else {
                                            panic!("expected string for I/O \"module\" value");
                                        };
                                        module = Some(s.clone());
                                    }
                                    "plugin" => {
                                        let toml::Value::String(s) = value else {
                                            panic!("expected string for I/O \"plugin\" value");
                                        };
                                        plugin = Some(s.clone());
                                    }
                                    "config" => {
                                        let toml::Value::Table(config_options) = value else {
                                            panic!("\"config\" option for I/O location must be a table")
                                        };

                                        config = Some(config_options);
                                    }
                                    _ => panic!("unrecognized key \"io.{}.{}\"", io_key, key)
                                }
                            }

                            // push back method, module, plugin here
                        },
                        _ => panic!("unrecognized value for key \"io.{}\"", io_key),
                    }
                }
            }
            _ => panic!("unrecognized top-level key {}", top_level_key.as_str()),
        }
    }
}
*/
