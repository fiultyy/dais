//! P-ARC R8: 原 app/src/terminal/event.rs 的 executor 命令完成事件(纯数据)随迁。
//! 宿主 app/src/terminal/event.rs re-export 保持旧路径兼容。

use std::fmt::{self, Debug, Formatter};
use std::num::ParseIntError;
use std::string::FromUtf8Error;

use warp_util::AsciiDebug;

/// Emitted upon completion of an executor command that goes through the pty, such as the
/// InBandCommandExecutor or the TmuxCommandExecutor.
#[derive(Clone)]
pub struct ExecutedExecutorCommandEvent {
    pub command_id: String,
    pub exit_code: usize,
    pub output: Vec<u8>,
}

impl ExecutedExecutorCommandEvent {
    /// Parses the given `payload` (expected to be the payload of a generator output OSC) into a
    /// `ExecutedGeneratorCommandValue`.
    ///
    /// The given `string` is expected to follow the following format:
    ///     <commmand_id>;<output>;<exit_code>
    ///
    /// Returns a `ParseGeneratorCommandValueError` if payload cannot be successfully parsed.
    ///
    pub fn parse_generator_payload(payload: Vec<u8>) -> Result<Self, ParseGeneratorOutputError> {
        // Break the payload apart at the first and last semicolons.
        let mut payload_initial_split = payload.splitn(2, |&byte| byte == b';');

        let Some(before_first_semicolon) = payload_initial_split.next() else {
            return Err(ParseGeneratorOutputError::Corrupted);
        };

        let Some(after_first_semicolon) = payload_initial_split.next() else {
            return Err(ParseGeneratorOutputError::Corrupted);
        };

        let mut payload_final_split = after_first_semicolon.rsplitn(2, |&byte| byte == b';');
        let Some(after_final_semicolon) = payload_final_split.next() else {
            return Err(ParseGeneratorOutputError::Corrupted);
        };

        let Some(payload_middle) = payload_final_split.next() else {
            return Err(ParseGeneratorOutputError::Corrupted);
        };

        let command_id = String::from_utf8(before_first_semicolon.to_vec())
            .map_err(ParseGeneratorOutputError::Utf8DecodingFailure)?;

        let exit_code = String::from_utf8(after_final_semicolon.to_vec())
            .map_err(ParseGeneratorOutputError::Utf8DecodingFailure)?
            .parse::<usize>()
            .map_err(ParseGeneratorOutputError::ExitCodeParseFailure)?;

        // The output of the command remains as bytes. This is so we can operate on the bytes higher in
        // the stack if we need to, such as in the case of parsing out the zsh history file where we want to
        // transform the byte array before converting to a string.
        let output = payload_middle.to_vec();

        Ok(Self {
            command_id,
            exit_code,
            output,
        })
    }
}

impl Debug for ExecutedExecutorCommandEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutedExecutorCommandEvent")
            .field("command_id", &self.command_id)
            .field("exit_code", &self.exit_code)
            .field("output", &AsciiDebug(&self.output))
            .finish()
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ParseGeneratorOutputError {
    #[error("Failed to parse exit code: {0:?}")]
    ExitCodeParseFailure(ParseIntError),
    #[error("Corrupted DCS. Should be of the format <command_id>;<exit_code>;<output>. ")]
    Corrupted,
    #[error("Failed to convert to Utf8: {0:?}")]
    Utf8DecodingFailure(FromUtf8Error),
}

