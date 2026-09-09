use crate::signatures::{Command, Priority};

use super::*;

fn test_signature() -> CommandSignature {
    CommandSignature {
        command: Command {
            name: "test".to_owned(),
            alias: vec![],
            description: None,
            arguments: vec![],
            subcommands: vec![],
            options: vec![],
            priority: Priority::default(),
        },
    }
}

#[test]
fn test_command_registry_registers_signature() {
    let registry = CommandRegistry::new();
    registry.register_signature(test_signature());

    let signature = registry
        .get_signature("test")
        .expect("Signature is registered.");
    assert_eq!(signature.command.name, "test");
}

#[test]
fn test_registered_commands_releases_registry_lock() {
    // Regression test for a deadlock: `registered_commands()` used to return a
    // memo-map `keys()` iterator that holds the registry's internal mutex for
    // as long as the iterator lives. Re-entering the registry while the
    // iterator is alive (as `sorted_top_level_commands` does via
    // `command_suggestion` -> `get_signature`) deadlocked on the non-reentrant
    // mutex. Run the re-entry on a side thread with a timeout so a regression
    // fails fast instead of hanging the test process.
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::Duration;

    let registry = Arc::new(CommandRegistry::new());
    registry.register_signature(test_signature());

    let commands = registry.registered_commands();

    // Re-enter the registry while the iterator is still alive. Before the
    // snapshot fix this blocked forever on the registry mutex.
    let (tx, rx) = mpsc::channel();
    let reentrant_registry = Arc::clone(&registry);
    thread::spawn(move || {
        let found = reentrant_registry.get_signature("test").is_some();
        let _ = tx.send(found);
    });
    let found = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("get_signature deadlocked: registered_commands() still holds the registry lock");
    assert!(found);

    // The snapshot preserves the set of registered commands.
    let commands: Vec<_> = commands.collect();
    assert_eq!(commands, vec!["test"]);
}
