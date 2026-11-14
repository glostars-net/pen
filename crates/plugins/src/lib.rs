//! Plugin host. A plugin may register a command name and a theme JSON string.
//! It cannot read the disk, open a socket, or draw UI.

use std::sync::OnceLock;

use thiserror::Error;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Engine, Store};

mod bindings {
    wasmtime::component::bindgen!({
        world: "pen",
        path: "wit",
    });
}

/// Copied from `pen::palette::BUILTIN_COMMANDS`.
/// shortcut: two lists, update both when a built-in command is added.
const BUILTIN_COMMANDS: &[&str] = &[
    "Open folder",
    "Save",
    "Find",
    "Stage file",
    "Discard file",
    "Commit",
    "Fetch",
    "Push",
    "Merge",
    "Add worktree",
    "Split diff",
    "Use Ink",
    "Use Paper",
    "Go to definition",
];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PluginError {
    #[error("{0}")]
    Rejected(String),
    #[error("plugin failed to load")]
    Load,
    #[error("plugin trapped")]
    Trap,
}

pub struct Host {
    pub commands: Vec<String>,
    pub themes: Vec<String>,
}

impl Host {
    /// Reject an empty name and any built-in command, case-sensitive.
    pub fn register_command(&mut self, name: &str) -> Result<(), PluginError> {
        accept_command(&mut self.commands, name)
    }

    /// Store the raw JSON. Parsing happens later, in `accepted_themes`.
    pub fn register_theme(&mut self, json: &str) -> Result<(), PluginError> {
        self.themes.push(json.to_owned());
        Ok(())
    }

    /// Instantiate a component and call `init`. No WASI. Fuel is 1_000_000.
    pub fn load(&mut self, wasm: &[u8]) -> Result<(), PluginError> {
        let engine = engine()?;
        let component = Component::from_binary(engine, wasm).map_err(|_| PluginError::Load)?;
        let mut linker = Linker::new(engine);
        bindings::Pen::add_to_linker::<State, HasSelf<State>>(&mut linker, |state| state)
            .map_err(|_| PluginError::Load)?;
        let mut store = Store::new(engine, State::default());
        store.set_fuel(1_000_000).map_err(|_| PluginError::Load)?;
        let instance = bindings::Pen::instantiate(&mut store, &component, &linker)
            .map_err(|_| PluginError::Load)?;
        match instance.call_init(&mut store) {
            Ok(()) => {
                let state = store.data_mut();
                self.commands.append(&mut state.commands);
                self.themes.append(&mut state.themes);
                Ok(())
            }
            Err(_) => Err(PluginError::Trap),
        }
    }
}

fn engine() -> Result<&'static Engine, PluginError> {
    static ENGINE: OnceLock<Result<Engine, ()>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let mut config = wasmtime::Config::new();
            config.wasm_component_model(true);
            config.consume_fuel(true);
            Engine::new(&config).map_err(|_| ())
        })
        .as_ref()
        .map_err(|_| PluginError::Load)
}

#[derive(Default)]
struct State {
    commands: Vec<String>,
    themes: Vec<String>,
}

/// Themes whose JSON `load_theme` accepts. Ink and Paper are not included.
pub fn accepted_themes(raw: &[String]) -> Vec<highlight::Theme> {
    raw.iter()
        .filter_map(|json| highlight::load_theme(json).ok())
        .collect()
}

/// Load every `*.wasm` in `dir`. A missing directory adds nothing.
/// The first trap is reported as `Plugin {name} stopped.`
pub fn load_dir(host: &mut Host, dir: &std::path::Path) -> Option<String> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return None;
    };
    let mut paths: Vec<_> = read
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    paths.retain(|path| path.extension().is_some_and(|ext| ext == "wasm"));
    paths.sort();
    let mut note = None;
    for path in paths {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if let Err(PluginError::Trap) = host.load(&bytes)
            && note.is_none()
        {
            let name = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| "plugin".into());
            note = Some(format!("Plugin {name} stopped."));
        }
    }
    note
}

fn accept_command(commands: &mut Vec<String>, name: &str) -> Result<(), PluginError> {
    if name.is_empty() || BUILTIN_COMMANDS.contains(&name) {
        return Err(PluginError::Rejected(name.to_owned()));
    }
    commands.push(name.to_owned());
    Ok(())
}

impl bindings::pen::host::host::Host for State {
    fn register_command(&mut self, name: String) -> Result<(), String> {
        accept_command(&mut self.commands, &name).map_err(|e| e.to_string())
    }

    fn register_theme(&mut self, json: String) -> Result<(), String> {
        self.themes.push(json);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn builtin_command_is_rejected() {
        let mut host = Host {
            commands: vec![],
            themes: vec![],
        };
        assert!(host.register_command("Save").is_err());
        assert!(host.commands.is_empty());
    }

    #[test]
    fn load_hello_registers_command() {
        let mut host = Host {
            commands: vec![],
            themes: vec![],
        };
        host.load(include_bytes!("../tests/hello.wasm"))
            .expect("hello component loads");
        assert_eq!(host.commands, vec!["Hello".to_owned()]);
    }

    #[test]
    fn core_module_is_not_a_component() {
        let mut host = Host {
            commands: vec![],
            themes: vec![],
        };
        assert_eq!(host.load(b"\0asm\x01\0\0\0"), Err(PluginError::Load));
        assert!(host.commands.is_empty());
    }

    #[test]
    fn bad_theme_json_is_skipped() {
        let ok = r##"{"name":"Night","bg":"#111111","fg":"#eeeeee"}"##;
        let themes = accepted_themes(&[ok.into(), "not json".into()]);
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name, "Night");
    }

    #[test]
    fn missing_plugins_dir_adds_nothing() {
        let mut host = Host {
            commands: vec![],
            themes: vec![],
        };
        let note = load_dir(&mut host, Path::new("/no/such/pen-plugins"));
        assert!(note.is_none());
        assert!(host.commands.is_empty());
        assert!(host.themes.is_empty());
    }

    #[test]
    fn trap_does_not_kill_the_host() {
        let mut host = Host {
            commands: vec![],
            themes: vec![],
        };
        assert_eq!(
            host.load(include_bytes!("../tests/spin.wasm")),
            Err(PluginError::Trap)
        );
        host.register_command("After")
            .expect("host still accepts commands");
        assert_eq!(host.commands, vec!["After".to_owned()]);
    }

    #[test]
    fn empty_command_is_rejected() {
        let mut host = Host {
            commands: vec![],
            themes: vec![],
        };
        assert!(host.register_command("").is_err());
        assert!(host.commands.is_empty());
    }
}
