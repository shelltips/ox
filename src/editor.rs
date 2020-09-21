// Editor.rs - Controls the editor and brings everything together
use crate::config::{BG, FG, LINE_NUMBER_FG, RESET_BG, RESET_FG, STATUS_BG, STATUS_FG, TAB_WIDTH};
use crate::util::{is_ahead, is_behind, raw_to_grapheme, title}; // Bring in the utils
use crate::{Document, Event, Row, Terminal}; // Bringing in all the structs
use std::time::Duration; // For implementing an FPS cap
use std::{cmp, env, thread}; // Managing threads, arguments and comparisons.
use termion::event::Key; // For reading Keys and shortcuts
use termion::input::TermRead; // To allow reading from the terminal
use termion::{color, style}; // For managing colors and styles of text
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr}; // For calculating unicode character widths

// Get the current version of Ox
const VERSION: &str = env!("CARGO_PKG_VERSION");

// Enum for the kinds of status messages
enum Type {
    Error,
    Warning,
    Info,
}

// Enum for holding prompt events
enum PromptEvent {
    Update,
    CharPress,
    KeyPress(Key),
}

// For holding the info in the command line
struct CommandLine {
    msg: Type,
    text: String,
}

// For representing positions
#[derive(Clone, Copy, Debug)]
pub struct Position {
    pub x: usize,
    pub y: usize,
}

// The main editor struct
pub struct Editor {
    quit: bool,                // Toggle for cleanly quitting the editor
    show_welcome: bool,        // Toggle for showing the welcome message
    dirty: bool,               // True if the current document has been edited
    graphemes: usize,          // For holding the special grapheme cursor
    command_line: CommandLine, // For holding the command line
    term: Terminal,            // For the handling of the terminal
    cursor: Position,          // For holding the raw cursor location
    doc: Document,             // For holding our document
    offset: Position,          // For holding the offset on the X and Y axes
}

// Implementing methods for our editor struct / class
impl Editor {
    pub fn new() -> Self {
        // Create a new editor instance
        let args: Vec<String> = env::args().collect();
        Self {
            quit: false,
            show_welcome: args.len() < 2,
            dirty: false,
            command_line: CommandLine {
                text: "Welcome to Ox!".to_string(),
                msg: Type::Info,
            },
            term: Terminal::new(),
            graphemes: 0,
            cursor: Position { x: 0, y: 0 },
            offset: Position { x: 0, y: 0 },
            doc: if args.len() >= 2 {
                Document::from(args[1].trim())
            } else {
                Document::new()
            },
        }
    }
    pub fn run(&mut self) {
        // Run the editor instance
        while !self.quit {
            self.update();
            self.process_input();
        }
    }
    fn read_key(&mut self) -> Key {
        // Wait until a key is pressed and then return it
        loop {
            let stdin = &mut self.term.stdin;
            if let Some(key) = stdin.keys().next() {
                // When a keypress was detected
                return key.unwrap();
            } else {
                // Run code that we want to run when the key isn't pressed
                if self.term.check_resize() {
                    // The terminal has changed in size
                    if self.cursor.y > self.term.height.saturating_sub(3) as usize {
                        // Prevent cursor going off the screen and breaking everything
                        self.cursor.y = self.term.height.saturating_sub(3) as usize;
                    }
                    // Re-render everything to the new size
                    self.update();
                }
                // FPS cap to stop using the entire CPU
                thread::sleep(Duration::from_millis(16));
            }
        }
    }
    fn process_input(&mut self) {
        // Read a key and act on it
        let key = self.read_key();
        match key {
            Key::Char(c) => self.character(c),
            Key::Backspace => self.backspace(),
            Key::Ctrl('q') => self.quit(),
            Key::Ctrl('s') => self.save(),
            Key::Ctrl('w') => self.save_as(),
            Key::Ctrl('n') => self.new_document(),
            Key::Ctrl('o') => self.open_document(),
            Key::Ctrl('f') => self.search(),
            Key::Ctrl('u') => self.undo(),
            Key::Ctrl('y') => self.redo(),
            Key::Left | Key::Right | Key::Up | Key::Down => self.move_cursor(key),
            Key::PageDown | Key::PageUp | Key::Home | Key::End => self.leap_cursor(key),
            _ => (),
        }
    }
    fn redo(&mut self) {
        if let Some(event) = self.doc.redo_stack.pop() {
            self.doc.undo_stack.push(event);
            match event {
                Event::InsertTab(pos) => {
                    self.cursor.y = pos.y - self.offset.y;
                    self.cursor.x = pos.x.saturating_sub(TAB_WIDTH) - self.offset.x;
                    self.recalculate_graphemes();
                    self.tab();
                },
                Event::InsertMid(pos, c) => {
                    let c_len = UnicodeWidthChar::width(c).map_or(0, |c| c);
                    self.cursor.y = pos.y - self.offset.y;
                    self.cursor.x = pos.x.saturating_add(c_len) - self.offset.x;
                    self.doc.rows[pos.y].insert(c, pos.x);
                }
                _ => (),
            }
            self.set_command_line(format!("{:?}", event), Type::Info);
        } else {
            self.set_command_line("Empty Redo Stack".to_string(), Type::Error);
        }
    }
    fn undo(&mut self) {
        if let Some(event) = self.doc.undo_stack.pop() {
            match event {
                Event::InsertTab(pos) => {
                    for i in 1..=TAB_WIDTH {
                        self.doc.rows[pos.y].delete(pos.x - i);
                        self.move_cursor(Key::Left);
                    }
                }
                Event::InsertMid(pos, c) => {
                    let c_len = UnicodeWidthChar::width(c).map_or(0, |c| c);
                    self.cursor.y = pos.y - self.offset.y;
                    self.cursor.x = pos.x.saturating_add(c_len) - self.offset.x;
                    self.recalculate_graphemes();
                    let string = self.doc.rows[pos.y].string.clone();
                    self.doc.rows[pos.y].delete(raw_to_grapheme(pos.x, &string));
                    for _ in 0..c_len {
                        self.move_cursor(Key::Left);
                    }
                }
                Event::BackspaceMid(pos, c) => {
                    self.doc.rows[pos.y].insert(c, pos.x);
                    self.move_cursor(Key::Right);
                }
                Event::ReturnEnd(pos) => {
                    self.doc.rows.remove(pos.y + 1);
                    self.move_cursor(Key::Up);
                    self.leap_cursor(Key::End);
                }
                Event::ReturnStart(pos) => {
                    self.doc.rows.remove(pos.y);
                    self.move_cursor(Key::Up);
                }
                Event::ReturnMid(pos, breakpoint) => {
                    let current = self.doc.rows[pos.y].string.clone();
                    let after = self.doc.rows[pos.y + 1].string.clone();
                    self.doc.rows.remove(pos.y);
                    self.doc.rows[pos.y] = Row::from(&(current + &after)[..]);
                    self.move_cursor(Key::Up);
                    self.leap_cursor(Key::Home);
                    for _ in 0..breakpoint {
                        self.move_cursor(Key::Right);
                    }
                }
                Event::BackspaceStart(pos) => {
                    let before = Row::from(&self.doc.rows[pos.y].string[..pos.x]);
                    let after = Row::from(&self.doc.rows[pos.y].string[pos.x..]);
                    self.doc.rows[pos.y] = after;
                    self.doc.rows.insert(pos.y, before);
                    self.move_cursor(Key::Down);
                    self.leap_cursor(Key::Home);
                }
            }
            self.doc.redo_stack.push(event);
            self.set_command_line(format!("{:?}", event), Type::Info);
        } else {
            self.set_command_line("Empty Undo Stack".to_string(), Type::Error);
        }
    }
    fn set_command_line(&mut self, text: String, msg: Type) {
        self.command_line = CommandLine { text, msg };
    }
    fn character(&mut self, c: char) {
        // The user pressed a character key
        self.dirty = true;
        self.show_welcome = false;
        match c {
            '\n' => self.return_key(), // The user pressed the return key
            '\t' => {
                // The user pressed the tab key
                self.tab();
                self.doc.undo_stack.push(Event::InsertTab(Position {
                    x: self.cursor.x + self.offset.x,
                    y: self.cursor.y + self.offset.y,
                }));
            }
            _ => {
                // Other characters
                self.dirty = true;
                self.show_welcome = false;
                self.doc.rows[self.cursor.y + self.offset.y].insert(c, self.graphemes);
                self.doc.undo_stack.push(Event::InsertMid(
                    Position {
                        x: self.cursor.x + self.offset.x,
                        y: self.cursor.y + self.offset.y,
                    },
                    c,
                ));
                self.move_cursor(Key::Right);
            }
        }
        self.doc.redo_stack.empty();
    }
    fn tab(&mut self) {
        for _ in 0..TAB_WIDTH {
            self.doc.rows[self.cursor.y + self.offset.y].insert(' ', self.graphemes);
            self.move_cursor(Key::Right);
        }
    }
    fn backspace(&mut self) {
        // Handling the backspace key
        self.dirty = true;
        self.show_welcome = false;
        if self.cursor.x + self.offset.x == 0 && self.cursor.y + self.offset.y != 0 {
            // Backspace at the start of a line
            let current = self.doc.rows[self.cursor.y + self.offset.y].string.clone();
            let prev = self.doc.rows[self.cursor.y + self.offset.y - 1].clone();
            self.doc.rows[self.cursor.y + self.offset.y - 1] =
                Row::from(&(prev.string.clone() + &current)[..]);
            self.doc.rows.remove(self.cursor.y + self.offset.y);
            self.move_cursor(Key::Up);
            self.cursor.x = prev.length();
            self.recalculate_graphemes();
            self.doc.undo_stack.push(Event::BackspaceStart(Position {
                x: self.cursor.x + self.offset.x,
                y: self.cursor.y + self.offset.y,
            }));
        } else {
            // Backspace in the middle of a line
            self.move_cursor(Key::Left);
            let ch = self.doc.rows[self.cursor.y + self.offset.y].clone();
            self.doc.rows[self.cursor.y + self.offset.y].delete(self.graphemes);
            if let Some(ch) = ch.chars().get(self.graphemes) {
                self.doc.undo_stack.push(Event::BackspaceMid(
                    Position {
                        x: self.cursor.x + self.offset.x,
                        y: self.cursor.y + self.offset.y,
                    },
                    ch.parse().unwrap(),
                ));
            }
        }
    }
    fn quit(&mut self) {
        // For handling a quit event
        if self.dirty_prompt('q', "quit") {
            self.quit = true;
        }
    }
    fn new_document(&mut self) {
        // Handle new document event
        if self.dirty_prompt('n', "new") {
            self.doc = Document::new();
            self.dirty = false;
            self.cursor.y = 0;
            self.offset.y = 0;
            self.leap_cursor(Key::Home);
        }
    }
    fn open_document(&mut self) {
        // Handle new document event
        if self.dirty_prompt('o', "open") {
            if let Some(result) = self.prompt("Open", &|_, _, _| {}) {
                if let Some(doc) = Document::open(&result[..]) {
                    self.doc = doc;
                    self.dirty = false;
                    self.show_welcome = false;
                    self.cursor.y = 0;
                    self.offset.y = 0;
                    self.leap_cursor(Key::Home);
                } else {
                    self.set_command_line("File couldn't be opened".to_string(), Type::Error);
                }
            }
        } else {
            self.set_command_line("Open cancelled".to_string(), Type::Info);
        }
    }
    fn save(&mut self) {
        // Handle save event
        if self.doc.save().is_ok() {
            self.dirty = false;
            self.set_command_line(
                format!("File saved to {} successfully", self.doc.path),
                Type::Info,
            );
        } else {
            self.set_command_line(
                format!("Failed to save file to {}", self.doc.path),
                Type::Error,
            );
        }
    }
    fn save_as(&mut self) {
        // Handle save as event
        if let Some(result) = self.prompt("Save as", &|_, _, _| {}) {
            if self.doc.save_as(&result[..]).is_ok() {
                self.dirty = false;
                self.set_command_line(format!("File saved to {} successfully", result), Type::Info);
                self.doc.name = result.clone();
                self.doc.path = result;
            } else {
                self.set_command_line(format!("Failed to save file to {}", result), Type::Error);
            }
        } else {
            self.set_command_line("Save as cancelled".to_string(), Type::Info);
        }
    }
    fn search(&mut self) {
        // For searching the file
        let initial_cursor = self.cursor;
        let initial_offset = self.offset;
        self.prompt("Search", &|s, e, t| {
            let search_points = s.doc.scan(t);
            match e {
                PromptEvent::KeyPress(k) => match k {
                    Key::Left | Key::Up => {
                        for p in search_points.iter().rev() {
                            if is_behind(&s.cursor, &s.offset, &p) {
                                s.goto_behind(&p);
                                s.recalculate_graphemes();
                                break;
                            }
                        }
                    }
                    Key::Right | Key::Down => {
                        for p in search_points {
                            if is_ahead(&s.cursor, &s.offset, &p) {
                                s.goto_ahead(&p);
                                s.recalculate_graphemes();
                                break;
                            }
                        }
                    }
                    Key::Esc => {
                        s.cursor = initial_cursor;
                        s.offset = initial_offset;
                        s.recalculate_graphemes();
                    }
                    _ => (),
                },
                PromptEvent::CharPress => {
                    s.cursor = initial_cursor;
                    s.offset = initial_offset;
                    if t != "" {
                        for p in search_points {
                            if is_ahead(&s.cursor, &s.offset, &p) {
                                s.goto_ahead(&p);
                                s.recalculate_graphemes();
                                break;
                            }
                        }
                    }
                }
                PromptEvent::Update => (),
            }
        });
        self.set_command_line("Search exited".to_string(), Type::Info);
    }
    fn return_key(&mut self) {
        // Return key
        self.dirty = true;
        self.show_welcome = false;
        if self.cursor.x + self.offset.x == 0 {
            // Return key pressed at the start of the line
            self.doc
                .rows
                .insert(self.cursor.y + self.offset.y, Row::from(""));
            self.doc.undo_stack.push(Event::ReturnStart(Position {
                x: self.cursor.x + self.offset.x,
                y: self.cursor.y + self.offset.y,
            }));
            self.move_cursor(Key::Down);
        } else if self.cursor.x + self.offset.x
            == self.doc.rows[self.cursor.y + self.offset.y].length()
        {
            // Return key pressed at the end of the line
            self.doc
                .rows
                .insert(self.cursor.y + self.offset.y + 1, Row::from(""));
            self.doc.undo_stack.push(Event::ReturnEnd(Position {
                x: self.cursor.x + self.offset.x,
                y: self.cursor.y + self.offset.y,
            }));
            self.move_cursor(Key::Down);
            self.leap_cursor(Key::Home);
            self.recalculate_graphemes();
        } else {
            // Return key pressed in the middle of the line
            let current = self.doc.rows[self.cursor.y + self.offset.y].chars();
            let before = Row::from(&current[..self.graphemes].join("")[..]);
            let after = Row::from(&current[self.graphemes..].join("")[..]);
            self.doc
                .rows
                .insert(self.cursor.y + self.offset.y + 1, after);
            self.doc.rows[self.cursor.y + self.offset.y] = before.clone();
            self.doc.undo_stack.push(Event::ReturnMid(
                Position {
                    x: self.cursor.x + self.offset.x,
                    y: self.cursor.y + self.offset.y,
                },
                before.length(),
            ));
            self.move_cursor(Key::Down);
            self.leap_cursor(Key::Home);
        }
    }
    fn dirty_prompt(&mut self, key: char, subject: &str) -> bool {
        // For events that require changes to the document
        if self.dirty {
            // Handle unsaved changes
            self.set_command_line(
                format!(
                    "Unsaved Changes! Ctrl + {} to force {}",
                    key.to_uppercase(),
                    subject
                ),
                Type::Warning,
            );
            self.update();
            match self.read_key() {
                Key::Char('\n') => return true,
                Key::Ctrl(k) => {
                    if k == key {
                        return true;
                    } else {
                        self.set_command_line(format!("{} cancelled", title(subject)), Type::Info);
                    }
                }
                _ => self.set_command_line(format!("{} cancelled", title(subject)), Type::Info),
            }
        } else {
            return true;
        }
        false
    }
    fn prompt(
        &mut self,
        prompt: &str,
        func: &dyn Fn(&mut Self, PromptEvent, &str),
    ) -> Option<String> {
        // Create a new prompt
        self.set_command_line(format!("{}: ", prompt), Type::Info);
        self.update();
        let mut result = String::new();
        'p: loop {
            let key = self.read_key();
            match key {
                Key::Char(c) => {
                    if c == '\n' {
                        break 'p;
                    } else {
                        result.push(c);
                    }
                    func(self, PromptEvent::CharPress, &result)
                }
                Key::Backspace => {
                    result.pop();
                    func(self, PromptEvent::CharPress, &result)
                }
                Key::Esc => {
                    func(self, PromptEvent::KeyPress(key), &result);
                    return None;
                }
                _ => func(self, PromptEvent::KeyPress(key), &result),
            }
            self.set_command_line(format!("{}: {}", prompt, result), Type::Info);
            func(self, PromptEvent::Update, &result);
            self.update();
        }
        Some(result)
    }
    fn leap_cursor(&mut self, action: Key) {
        match action {
            Key::PageUp => {
                self.cursor.y = 0;
                self.snap_cursor();
                self.prevent_unicode_hell();
                self.recalculate_graphemes();
            }
            Key::PageDown => {
                self.cursor.y = cmp::min(
                    self.doc.rows.len().saturating_sub(1),
                    self.term.height.saturating_sub(3) as usize,
                );
                self.snap_cursor();
                self.prevent_unicode_hell();
                self.recalculate_graphemes();
            }
            Key::Home => {
                self.offset.x = 0;
                self.cursor.x = 0;
                self.graphemes = 0;
            }
            Key::End => {
                let line = &self.doc.rows[self.cursor.y + self.offset.y];
                if line.length()
                    >= self.term.width.saturating_sub(self.doc.line_offset as u16) as usize
                {
                    // Work out the width of the character to traverse
                    let mut jump = 1;
                    if let Some(chr) = line.ext_chars().get(line.length()) {
                        jump = UnicodeWidthStr::width(*chr);
                    }
                    self.offset.x = line
                        .length()
                        .saturating_add(jump + self.doc.line_offset + 1)
                        .saturating_sub(self.term.width as usize);
                    self.cursor.x = self
                        .term
                        .width
                        .saturating_sub((jump + self.doc.line_offset + 1) as u16)
                        as usize;
                } else {
                    self.cursor.x = line.length();
                }
                self.graphemes = line.chars().len();
            }
            _ => (),
        }
    }
    fn move_cursor(&mut self, direction: Key) {
        // Move the cursor around the editor
        match direction {
            Key::Down => {
                if self.cursor.y + self.offset.y + 1 < self.doc.rows.len() {
                    // If the proposed move is within the length of the document
                    if self.cursor.y == self.term.height.saturating_sub(3) as usize {
                        self.offset.y = self.offset.y.saturating_add(1);
                    } else {
                        self.cursor.y = self.cursor.y.saturating_add(1);
                    }
                    self.snap_cursor();
                    self.prevent_unicode_hell();
                    self.recalculate_graphemes();
                }
            }
            Key::Up => {
                if self.cursor.y == 0 {
                    self.offset.y = self.offset.y.saturating_sub(1);
                } else {
                    self.cursor.y = self.cursor.y.saturating_sub(1);
                }
                self.snap_cursor();
                self.prevent_unicode_hell();
                self.recalculate_graphemes();
            }
            Key::Right => {
                let line = &self.doc.rows[self.cursor.y + self.offset.y];
                // Work out the width of the character to traverse
                let mut jump = 1;
                if let Some(chr) = line.ext_chars().get(self.cursor.x + self.offset.x) {
                    jump = UnicodeWidthStr::width(*chr);
                }
                // Check the proposed move is within the current line length
                if line.length() > self.cursor.x + self.offset.x {
                    // Check for normal width character
                    let indicator1 = self.cursor.x
                        == self
                            .term
                            .width
                            .saturating_sub((self.doc.line_offset + jump + 1) as u16)
                            as usize;
                    // Check for half broken unicode character
                    let indicator2 = self.cursor.x
                        == self
                            .term
                            .width
                            .saturating_sub((self.doc.line_offset + jump) as u16)
                            as usize;
                    if indicator1 || indicator2 {
                        self.offset.x = self.offset.x.saturating_add(jump);
                    } else {
                        self.cursor.x = self.cursor.x.saturating_add(jump);
                    }
                    self.graphemes = self.graphemes.saturating_add(1);
                }
            }
            Key::Left => {
                let line = &self.doc.rows[self.cursor.y + self.offset.y];
                // Work out the width of the character to traverse
                let mut jump = 1;
                if let Some(chr) = line
                    .ext_chars()
                    .get((self.cursor.x + self.offset.x).saturating_sub(1))
                {
                    jump = UnicodeWidthStr::width(*chr);
                }
                if self.cursor.x == 0 {
                    self.offset.x = self.offset.x.saturating_sub(jump);
                } else {
                    self.cursor.x = self.cursor.x.saturating_sub(jump);
                }
                self.graphemes = self.graphemes.saturating_sub(1);
            }
            _ => (),
        }
    }
    fn snap_cursor(&mut self) {
        // Snap the cursor to the end of the row when outside
        let current = self.doc.rows[self.cursor.y + self.offset.y].clone();
        if current.length() <= self.cursor.x + self.offset.x {
            // If the cursor is out of bounds
            self.leap_cursor(Key::Home);
            self.leap_cursor(Key::End);
        }
    }
    fn prevent_unicode_hell(&mut self) {
        // Make sure that the cursor isn't inbetween a unicode character
        let line = &self.doc.rows[self.cursor.y + self.offset.y];
        if line.length() > self.cursor.x + self.offset.x {
            // As long as the cursor is within range
            let boundaries = line.boundaries();
            let mut index = self.cursor.x + self.offset.x;
            if !boundaries.contains(&index) && index != 0 {}
            while !boundaries.contains(&index) && index != 0 {
                self.cursor.x = self.cursor.x.saturating_sub(1);
                self.graphemes = self.graphemes.saturating_sub(1);
                index = index.saturating_sub(1);
            }
        }
    }
    fn recalculate_graphemes(&mut self) {
        // Recalculate the grapheme cursor after moving up and down
        let current = self.doc.rows[self.cursor.y + self.offset.y].clone();
        let jumps = current.get_jumps();
        let mut counter = 0;
        for (mut counter2, i) in jumps.into_iter().enumerate() {
            if counter == self.cursor.x + self.offset.x {
                break;
            }
            counter2 += 1;
            self.graphemes = counter2;
            counter += i;
        }
    }
    fn goto_behind(&mut self, position: &Position) {
        // Adjust the offset to ensure that the cursor is in view
        let max_range = self.term.height.saturating_sub(3) as usize;
        let max_x = (self.term.width as usize).saturating_sub(self.doc.line_offset);
        if self.offset.y == 0 && position.y < max_range {
            self.cursor = *position;
        } else {
            if position.x > max_x {
                self.offset.x = position.x;
            } else {
                self.cursor.x = position.x;
            }
            if position.y < max_range {
                self.offset.y = 0;
                self.cursor.y = position.y;
            } else {
                self.offset.y = position.y;
                self.cursor.y = 0;
            }
        }
    }
    fn goto_ahead(&mut self, position: &Position) {
        // Adjust the offset to ensure that the cursor is in view
        let max_range = self.term.height.saturating_sub(3) as usize;
        let max_x = (self.term.width as usize).saturating_sub(self.doc.line_offset);
        if self.offset.y == 0 && position.y < max_range {
            self.cursor = *position;
        } else {
            if position.x > max_x {
                self.offset.x = position.x;
            } else {
                self.cursor.x = position.x;
            }
            self.offset.y = position.y.saturating_sub(max_range);
            self.cursor.y = max_range;
        }
    }
    fn update(&mut self) {
        // Move the cursor and render the screen
        self.term.goto(&Position { x: 0, y: 0 });
        self.doc.recalculate_offset();
        self.render();
        self.term.goto(&Position {
            x: self.cursor.x.saturating_add(self.doc.line_offset + 1),
            y: self.cursor.y,
        });
        self.term.flush();
    }
    fn welcome_message(&self, text: &str, colour: color::Fg<color::Rgb>) -> String {
        let pad = " ".repeat((self.term.width as usize / 2).saturating_sub(text.len() / 2));
        let pad_right = " ".repeat(
            (self.term.width.saturating_sub(1) as usize).saturating_sub(text.len() + pad.len()),
        );
        format!(
            "{}{}~{}{}{}{}{}{}{}",
            BG, LINE_NUMBER_FG, RESET_FG, pad, colour, text, RESET_FG, pad_right, RESET_BG,
        )
    }
    fn status_line(&self) -> String {
        // Produce the status line
        // Create the left part of the status line
        let left = format!(
            " {}{} \u{2502} {}",
            self.doc.name,
            if self.dirty {
                "[+] \u{fb12} "
            } else {
                " \u{f723} "
            },
            self.doc.identify()
        );
        // Create the right part of the status line
        let right = format!(
            "\u{fa70} {} / {} \u{2502} \u{fae6}({}, {}) ",
            self.cursor.y + self.offset.y + 1,
            self.doc.rows.len(),
            self.cursor.x,
            self.cursor.y
        );
        // Get the padding value
        let padding = self.term.align_break(&left, &right);
        // Generate it
        format!(
            "{}{}{}{}{}{}{}{}{}",
            style::Bold,
            STATUS_FG,
            STATUS_BG,
            left,
            padding,
            right,
            RESET_BG,
            RESET_FG,
            style::Reset,
        )
    }
    fn add_background(&self, text: &str) -> String {
        // Add a background colour to a line
        format!("{}{}{}{}", BG, text, self.term.align_left(text), RESET_BG)
    }
    fn command_line(&self) -> String {
        // Render the command line
        let line = self.add_background(&self.command_line.text);
        // Add the correct styling
        match self.command_line.msg {
            Type::Error => format!(
                "{}{}{}{}{}",
                style::Bold,
                color::Fg(color::Red),
                line,
                color::Fg(color::Reset),
                style::Reset
            ),
            Type::Warning => format!(
                "{}{}{}{}{}",
                style::Bold,
                color::Fg(color::Yellow),
                line,
                color::Fg(color::Reset),
                style::Reset
            ),
            Type::Info => line,
        }
    }
    fn render(&mut self) {
        // Draw the screen to the terminal
        let mut frame = Vec::new();
        for row in 0..self.term.height {
            if row == self.term.height - 1 {
                // Render command line
                frame.push(self.command_line());
            } else if row == self.term.height - 2 {
                // Render status line
                frame.push(self.status_line());
            } else if row == self.term.height / 4 && self.show_welcome {
                frame.push(self.welcome_message(&format!("Ox editor  v{}", VERSION), FG));
            } else if row == (self.term.height / 4).saturating_add(1) && self.show_welcome {
                frame.push(self.welcome_message("A Rust powered editor by Luke", FG));
            } else if row == (self.term.height / 4).saturating_add(3) && self.show_welcome {
                frame.push(self.welcome_message("Ctrl + Q: Exit   ", STATUS_FG));
            } else if row == (self.term.height / 4).saturating_add(4) && self.show_welcome {
                frame.push(self.welcome_message("Ctrl + S: Save   ", STATUS_FG));
            } else if row == (self.term.height / 4).saturating_add(5) && self.show_welcome {
                frame.push(self.welcome_message("Ctrl + W: Save as", STATUS_FG));
            } else if let Some(line) = self.doc.rows.get(self.offset.y + row as usize) {
                // Render lines of code
                frame.push(self.add_background(&line.render(
                    self.offset.x,
                    self.term.width as usize,
                    self.offset.y + row as usize,
                    self.doc.line_offset,
                )));
            } else {
                // Render empty lines
                frame.push(format!(
                    "{}{}{}",
                    LINE_NUMBER_FG,
                    self.add_background("~"),
                    RESET_FG
                ));
            }
        }
        print!("{}", frame.join("\r\n"));
    }
}
