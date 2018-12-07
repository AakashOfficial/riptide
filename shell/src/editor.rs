use crate::buffer::Buffer;
use riptide::fd::*;
use std::borrow::Cow;
use std::io::Write;
use std::os::unix::io::*;
use termion::clear;
use termion::cursor;
use termion::event::Key;
use termion::input::TermRead;
use termion::raw::*;

/// The default prompt string if none is defined.
const DEFAULT_PROMPT: &str = "$ ";

/// Controls the interactive command line editor.
pub struct Editor {
    stdin: ReadPipe,
    stdout: WritePipe,
    _stderr: WritePipe,
    buffer: Buffer,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            stdin: unsafe { ReadPipe::from_raw_fd(0) },
            stdout: unsafe { WritePipe::from_raw_fd(0) },
            _stderr: unsafe { WritePipe::from_raw_fd(0) },
            buffer: Buffer::new(),
        }
    }

    pub fn read_line(&mut self) -> String {
        let prompt = self.get_prompt_str();
        write!(self.stdout, "{}", prompt).unwrap();
        self.stdout.flush().unwrap();

        // Duplicate stdin and stdout handles to workaround Termion's API.
        let stdin = self.stdin.clone();
        let stdout = self.stdout.clone();

        // Enter raw mode.
        let _raw_guard = stdout.into_raw_mode().unwrap();

        // Handle keyboard events.
        for key in stdin.keys() {
            match key.unwrap() {
                Key::Char('\n') => {
                    write!(self.stdout, "\r\n").unwrap();
                    break;
                }
                Key::Left => {
                    self.buffer.move_cursor_relative(-1);
                }
                Key::Right => {
                    self.buffer.move_cursor_relative(1);
                }
                Key::Home => {
                    self.buffer.move_to_start_of_line();
                }
                Key::End => {
                    self.buffer.move_to_end_of_line();
                }
                Key::Char(c) => {
                    self.buffer.insert_char(c);
                }
                Key::Backspace => {
                    self.buffer.delete_before_cursor();
                }
                Key::Delete => {
                    self.buffer.delete_after_cursor();
                }
                Key::Ctrl('c') => {
                    self.buffer.clear();
                }
                _ => {}
            }

            self.redraw();
        }

        // Move the command line out of out buffer and return it.
        self.buffer.take_text()
    }

    /// Redraw the buffer.
    pub fn redraw(&mut self) {
        let prompt = self.get_prompt_str();
        write!(self.stdout, "\r{}{}{}", clear::AfterCursor, prompt, self.buffer.text()).unwrap();

        // Update the cursor position.
        let diff = self.buffer.text().len() - self.buffer.cursor();
        if diff > 0 {
            write!(self.stdout, "{}", cursor::Left(diff as u16)).unwrap();
        }

        // Flush all changes from the IO buffer.
        self.stdout.flush().unwrap();
    }

    fn get_prompt_str(&self) -> Cow<'static, str> {
        // match interpreter::function_call(PROMPT_FUNCTION, &[], &mut Streams::null()) {
        //     Ok(Expression::Atom(s)) => s,
        //     _ => Cow::Borrowed(DEFAULT_PROMPT),
        // }

        Cow::Borrowed(DEFAULT_PROMPT)
    }
}
