use std::fs;

fn main() {}

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
