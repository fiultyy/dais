pub mod ai_agent_tasks;
pub mod blockgrid;
pub mod settings;
pub mod terminal;
mod virtual_fs;

pub use blockgrid::mock_blockgrid;
pub use terminal::add_window_with_terminal;
pub use virtual_fs::{Stub, VirtualFS};

macro_rules! assert_eventually {
    ($cond:expr, $($arg:tt)+) => {
        $crate::test_util::assert_eventually!(20 => $cond, $($arg)+);
    };
    // Run the condition up to ticks times, yielding to the executor in between.  If it does
    // not become true, this panics with the provided format string + args.
    ($ticks:literal => $cond:expr, $($arg:tt)+) => {{
        let mut pass = false;
        for _ in 0..$ticks {
            if $cond {
                pass = true;
                break;
            }
            warpui::r#async::Timer::after(std::time::Duration::from_millis(5)).await;
        }
        if !pass {
            panic!("{}", format_args!($($arg)+));
        }
    }};
}
pub(crate) use assert_eventually;

/// R9 共享测试装配: completions_v2 下 CommandRegistry::global_instance() 为空,
/// 单测注册断言涉及的命令签名(幂等 Once)。原 notebooks/editor 私有,提升共享。
pub fn initialize_command_signatures_for_tests() {
    use std::sync::Once;
    use warp_completer::signatures::{Command, CommandRegistry, CommandSignature, Opt};

    fn simple_command(name: &str) -> CommandSignature {
        CommandSignature {
            command: Command {
                name: name.to_owned(),
                description: Some(name.to_owned()),
                ..Default::default()
            },
        }
    }

    fn command_with_subcommand(
        name: &str,
        subcommand: &str,
        subcommand_options: Vec<Opt>,
    ) -> CommandSignature {
        CommandSignature {
            command: Command {
                name: name.to_owned(),
                description: Some(name.to_owned()),
                subcommands: vec![Command {
                    name: subcommand.to_owned(),
                    description: Some(subcommand.to_owned()),
                    options: subcommand_options,
                    ..Default::default()
                }],
                ..Default::default()
            },
        }
    }

    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let registry = CommandRegistry::global_instance();
        registry.register_signature(simple_command("echo"));
        registry.register_signature(simple_command("code"));
        registry.register_signature(simple_command("command"));
        registry.register_signature(command_with_subcommand(
            "git",
            "checkout",
            vec![Opt {
                name: vec!["-b".to_owned()],
                ..Default::default()
            }],
        ));
        registry.register_signature(command_with_subcommand("cargo", "run", vec![]));
    });
}
