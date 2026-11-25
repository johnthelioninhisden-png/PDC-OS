//! VGA/PS/2 terminal implementation.
//!
//! The terminal owns a fixed command-line buffer and dispatches entered lines
//! to the userspace shell/program registry.  Console hardware remains a kernel
//! service until ring-3 console devices and line disciplines are complete.

use super::keyboard::Key;
use crate::interrupts::IrqLock;
use crate::println;

const LINE_LIMIT: usize = 256;
const HISTORY_LIMIT: usize = 8;
const DEFAULT_COLOR: u8 = 0x07;
const PROMPT_COLOR: u8 = 0x0B;
const COMMAND_COLOR: u8 = 0x0A;

// The fixed line and history buffers avoid allocation in keyboard/terminal paths.
pub(crate) struct Terminal {
    line: [u8; LINE_LIMIT],
    length: usize,
    cursor: usize,
    foreground_pid: usize,
    history: [[u8; LINE_LIMIT]; HISTORY_LIMIT],
    history_lengths: [usize; HISTORY_LIMIT],
    history_count: usize,
    history_head: usize,
    history_position: usize,
}

impl Terminal {
    const fn new() -> Self {
        Self {
            line: [0; LINE_LIMIT],
            length: 0,
            cursor: 0,
            foreground_pid: 2,
            history: [[0; LINE_LIMIT]; HISTORY_LIMIT],
            history_lengths: [0; HISTORY_LIMIT],
            history_count: 0,
            history_head: 0,
            history_position: 0,
        }
    }

    fn reset_line(&mut self) {
        // Clear stale bytes as well as length so future buffer consumers cannot
        // accidentally observe characters from the previously executed line.
        for byte in &mut self.line {
            *byte = 0;
        }
        self.length = 0;
        self.cursor = 0;
    }

    fn add_history(&mut self) {
        if self.length == 0 {
            return;
        }

        // Do not duplicate the most recently entered identical command.
        if self.history_count > 0 {
            let newest = (self.history_head + HISTORY_LIMIT - 1) % HISTORY_LIMIT;
            if self.history_lengths[newest] == self.length
                && self.history[newest][..self.length] == self.line[..self.length]
            {
                self.history_position = 0;
                return;
            }
        }

        // The ring keeps the newest commands at the end and automatically
        // replaces the oldest command once HISTORY_LIMIT entries are retained.
        self.history[self.history_head][..self.length]
            .copy_from_slice(&self.line[..self.length]);
        self.history_lengths[self.history_head] = self.length;
        self.history_head = (self.history_head + 1) % HISTORY_LIMIT;
        if self.history_count < HISTORY_LIMIT {
            self.history_count += 1;
        }
        self.history_position = 0;
    }

    fn history_index(&self, position: usize) -> Option<usize> {
        if position == 0 || position > self.history_count {
            return None;
        }
        Some((self.history_head + HISTORY_LIMIT - position) % HISTORY_LIMIT)
    }

    fn load_history(&mut self, position: usize) {
        if position == 0 {
            self.reset_line();
            return;
        }
        let index = match self.history_index(position) {
            Some(index) => index,
            None => return,
        };

        // Replace the visible and buffered line in place, never by appending.
        let length = self.history_lengths[index];
        self.line[..length].copy_from_slice(&self.history[index][..length]);
        self.length = length;
        self.cursor = length;
        self.history_position = position;
    }

    fn erase_visible_line(&self) {
        // Move to the end of the line first, then erase it from right to left.
        // This works even when history navigation started in the middle of an
        // already visible command.
        for index in self.cursor..self.length {
            self.print_input_byte(index);
        }
        for _ in 0..self.length {
            crate::print!("\x08 \x08");
        }
    }

    fn redraw_visible_line(&self) {
        for index in 0..self.length {
            self.print_input_byte(index);
        }
        let remaining = self.length - self.cursor;
        for _ in 0..remaining {
            crate::print!("\x08");
        }
    }

    fn print_input_byte(&self, index: usize) {
        let command_end = self.line[..self.length]
            .iter()
            .position(|byte| byte.is_ascii_whitespace())
            .unwrap_or(self.length);
        let color = if index < command_end {
            COMMAND_COLOR
        } else {
            DEFAULT_COLOR
        };
        crate::userspace::terminal::set_color(color);
        crate::print!("{}", self.line[index] as char);
        crate::userspace::terminal::set_color(DEFAULT_COLOR);
    }
}

pub struct TerminalCell(IrqLock<Terminal>);

impl TerminalCell {
    const fn new() -> Self {
        Self(IrqLock::new(Terminal::new()))
    }

    pub(crate) fn with<R>(&self, action: impl FnOnce(&mut Terminal) -> R) -> R {
        let mut terminal = self.0.lock();
        action(&mut terminal)
    }
}

static TERMINAL: TerminalCell = TerminalCell::new();

pub fn init() {
    clear();
    print_banner();
    prompt();
}

pub fn clear() {
    crate::WRITER.lock().clear_screen();
}

pub(crate) fn set_color(color: u8) {
    crate::set_vga_color(color);
}

pub(crate) fn scrollback_page_up() {
    crate::scrollback_page_up();
}

pub(crate) fn scrollback_page_down() {
    crate::scrollback_page_down();
}

pub(crate) fn print_history() {
    TERMINAL.with(|terminal| {
        if terminal.history_count == 0 {
            println!("No commands in history");
            return;
        }
        let oldest =
            (terminal.history_head + HISTORY_LIMIT - terminal.history_count) % HISTORY_LIMIT;
        for position in 0..terminal.history_count {
            let index = (oldest + position) % HISTORY_LIMIT;
            let command = unsafe {
                core::str::from_utf8_unchecked(
                    &terminal.history[index][..terminal.history_lengths[index]],
                )
            };
            println!("{:>2}  {}", position + 1, command);
        }
    });
}

/// Update the fixed foreground-process identity used by console line discipline.
pub fn set_foreground_pid(pid: usize) {
    TERMINAL.with(|terminal| terminal.foreground_pid = pid);
}

pub fn foreground_pid() -> usize {
    TERMINAL.with(|terminal| terminal.foreground_pid)
}

fn print_banner() {
    println!("PDC-OS userspace shell (/bin/pdc-sh)");
    println!("Type 'help' for commands.");
}

fn prompt() {
    set_color(PROMPT_COLOR);
    crate::print!("pdc-os> ");
    set_color(DEFAULT_COLOR);
}

/// Compatibility entry point for callers that previously routed IRQ 1 here.
pub fn handle_keyboard_irq() {
    // Kept as a compatibility wrapper for callers that used the old API.
    // Production IRQ dispatch now calls keyboard::handle_irq() instead.
    super::keyboard::handle_irq();
}

/// Drain keyboard events outside interrupt context and update the shell.
pub fn poll_keyboard() {
    // The shell line discipline consumes input only while PID 2 (or whichever
    // process explicitly owns the console) is in the foreground.
    let foreground = foreground_pid();
    if crate::userspace::syscalls::foreground_console_pid() != foreground {
        return;
    }
    while let Some(key) = super::keyboard::try_pop() {
        handle_key(key);
    }
    if super::keyboard::take_wake_pending() && super::keyboard::overflow_count() != 0 {
        crate::serial::println(format_args!(
            "PS/2 console queue pressure: {} dropped events",
            super::keyboard::overflow_count()
        ));
    }
}

fn handle_key(key: Key) {
    match key {
        Key::Character(byte) => TERMINAL.with(|terminal| {
            // Insert in the cursor gap, reserve a terminator byte, and redraw
            // only the suffix so a full line remains editable in place.
            if terminal.length < LINE_LIMIT - 1 {
                for index in (terminal.cursor..terminal.length).rev() {
                    terminal.line[index + 1] = terminal.line[index];
                }
                terminal.line[terminal.cursor] = byte;
                terminal.length += 1;
                terminal.print_input_byte(terminal.cursor);
                for index in terminal.cursor + 1..terminal.length {
                    terminal.print_input_byte(index);
                }
                let remaining = terminal.length - (terminal.cursor + 1);
                for _ in 0..remaining {
                    crate::print!("\x08");
                }
                terminal.cursor += 1;
            }
        }),
        Key::Backspace => TERMINAL.with(|terminal| {
            if terminal.cursor > 0 {
                terminal.cursor -= 1;
                for index in terminal.cursor..terminal.length - 1 {
                    terminal.line[index] = terminal.line[index + 1];
                }
                terminal.length -= 1;
                terminal.line[terminal.length] = 0;

                // Move the framebuffer cursor onto the character being removed
                // before redrawing the suffix or clearing the old end column.
                crate::print!("\x08");
                for index in terminal.cursor..terminal.length {
                    terminal.print_input_byte(index);
                }
                set_color(DEFAULT_COLOR);
                crate::print!(" ");
                let remaining = terminal.length - terminal.cursor + 1;
                for _ in 0..remaining {
                    crate::print!("\x08");
                }
            }
        }),
        Key::Delete => TERMINAL.with(|terminal| {
            if terminal.cursor < terminal.length {
                for index in terminal.cursor..terminal.length - 1 {
                    terminal.line[index] = terminal.line[index + 1];
                }
                terminal.length -= 1;
                terminal.line[terminal.length] = 0;

                for index in terminal.cursor..terminal.length {
                    terminal.print_input_byte(index);
                }
                set_color(DEFAULT_COLOR);
                crate::print!(" ");
                let remaining = terminal.length - terminal.cursor + 1;
                for _ in 0..remaining {
                    crate::print!("\x08");
                }
            }
        }),
        Key::ArrowLeft => TERMINAL.with(|terminal| {
            if terminal.cursor > 0 {
                terminal.cursor -= 1;
                crate::print!("\x08");
            }
        }),
        Key::ArrowRight => TERMINAL.with(|terminal| {
            if terminal.cursor < terminal.length {
                terminal.print_input_byte(terminal.cursor);
                terminal.cursor += 1;
            }
        }),
        Key::Home => TERMINAL.with(|terminal| {
            for _ in 0..terminal.cursor {
                crate::print!("\x08");
            }
            terminal.cursor = 0;
        }),
        Key::End => TERMINAL.with(|terminal| {
            for index in terminal.cursor..terminal.length {
                terminal.print_input_byte(index);
            }
            terminal.cursor = terminal.length;
        }),
        Key::ArrowUp => TERMINAL.with(|terminal| {
            if terminal.history_count == 0 {
                return;
            }
            let next = terminal.history_position + 1;
            if next <= terminal.history_count {
                terminal.erase_visible_line();
                terminal.load_history(next);
                terminal.redraw_visible_line();
            }
        }),
        Key::ArrowDown => TERMINAL.with(|terminal| {
            if terminal.history_position > 1 {
                terminal.erase_visible_line();
                let next = terminal.history_position - 1;
                terminal.load_history(next);
                terminal.redraw_visible_line();
            } else if terminal.history_position == 1 {
                terminal.erase_visible_line();
                terminal.load_history(0);
            }
        }),
        Key::PageUp => scrollback_page_up(),
        Key::PageDown => scrollback_page_down(),
        Key::Enter => {
            let mut command = [0u8; LINE_LIMIT];
            let length = TERMINAL.with(|terminal| {
                crate::println!();
                if terminal.length == 0 {
                    return 0;
                }

                // Only decoded printable ASCII bytes enter the line buffer.
                // This permits a borrowed UTF-8 view without allocating a copy.
                terminal.add_history();
                let length = terminal.length;
                command[..length].copy_from_slice(&terminal.line[..length]);
                terminal.reset_line();
                length
            });
            if length != 0 {
                let line = unsafe { core::str::from_utf8_unchecked(&command[..length]) };
                super::commands::execute(line);
            }
            prompt();
        }
        Key::ControlC => TERMINAL.with(|terminal| {
            crate::print!("^C");
            crate::println!();
            terminal.reset_line();
            prompt();
        }),
        Key::ControlL => TERMINAL.with(|terminal| {
            clear();
            print_banner();
            terminal.reset_line();
            prompt();
        }),
        Key::Escape => {
            // Escape currently resets the visible shell, not its command state.
            clear();
            print_banner();
            prompt();
            TERMINAL.with(|terminal| terminal.reset_line());
        }
        Key::Release | Key::Unknown => {}
    }
}
