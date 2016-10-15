// Copyleft (ↄ) meh. <meh@schizofreni.co> | http://meh.schizofreni.co
//
// This file is part of cancer.
//
// cancer is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// cancer is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with cancer.  If not, see <http://www.gnu.org/licenses/>.

use std::rc::Rc;
use std::sync::Arc;
use std::io::Write;
use std::collections::VecDeque;
use std::iter;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use picto::Area;
use picto::color::Rgba;
use error;
use config::{self, Config};
use config::style::Shape;
use style::{self, Style};
use terminal::{Iter, Touched, Cell, Key, Action, cell};
use terminal::mode::{self, Mode};
use terminal::cursor::{self, Cursor};
use control::{self, Control, C0, C1, CSI, SGR};

#[derive(Debug)]
pub struct Terminal {
	config:  Arc<Config>,
	area:    Area,
	cache:   Option<Vec<u8>>,
	touched: Touched,

	mode:   Mode,
	rows:   VecDeque<VecDeque<Cell>>,
	scroll: Option<u32>,
	cursor: Cursor,
	saved:  Option<Cursor>,
}

macro_rules! term {
	($term:ident; row for $y:expr) => (
		($y + $term.scroll.unwrap_or_else(|| $term.rows.len() as u32 - $term.area.height)) as usize
	);

	($term:ident; extend) => (
		$term.rows.push_back(vec_deque![Cell::empty($term.cursor.style().clone()); $term.area.width as usize]);
	);

	($term:ident; cursor $($travel:tt)*) => (
		if let Some(n) = $term.cursor.travel(cursor::$($travel)*, &mut $term.touched) {
			$term.touched.all();

			for _ in 0 .. n {
				term!($term; extend);
			}
		}
	);

	($term:ident; touched all) => (
		$term.touched.all();
	);

	($term:ident; touched line $y:expr) => (
		$term.touched.line($y);
	);

	($term:ident; touched ($x:expr, $y:expr)) => (
		$term.touched.mark($x, $y);
	);

	($term:ident; touched $pair:expr) => (
		$term.touched.push($pair);
	);

	($term:ident; mut cell $x:expr, $y:expr) => ({
		let row = term!($term; row for $y);
		&mut $term.rows[row][$x as usize]
	})
}

impl Terminal {
	pub fn open(config: Arc<Config>, width: u32, height: u32) -> error::Result<Self> {
		let area  = Area::from(0, 0, width, height);
		let style = Rc::new(Style::default());
		let rows  = vec_deque![vec_deque![Cell::empty(style.clone()); width as usize]; height as usize];

		Ok(Terminal {
			config:  config.clone(),
			area:    area,
			cache:   Default::default(),
			touched: Touched::default(),

			mode:   Mode::default(),
			rows:   rows,
			scroll: None,
			cursor: Cursor::new(config.clone(), width, height),
			saved:  None,
		})
	}

	pub fn columns(&self) -> u32 {
		self.area.width
	}

	pub fn rows(&self) -> u32 {
		self.area.height
	}

	pub fn mode(&self) -> Mode {
		self.mode
	}

	/// Get the cursor.
	pub fn cursor(&self) -> cursor::Cell {
		cursor::Cell::new(&self.cursor, self.get(self.cursor.x(), self.cursor.y()))
	}

	/// Get the cell at the given position.
	pub fn get(&self, x: u32, y: u32) -> cell::Position {
		cell::Position::new(x, y, &self.rows[term!(self; row for y)][x as usize])
	}

	pub fn iter<'a, T: Iterator<Item = (u32, u32)>>(&'a self, iter: T) -> impl Iterator<Item = cell::Position<'a>> {
		Iter::new(self, iter)
	}

	/// Resize the terminal.
	pub fn resize(&mut self, width: u32, height: u32) -> impl Iterator<Item = (u32, u32)> {
		::std::iter::empty()
	}

	/// Enable or disable blinking and return the affected cells.
	pub fn blinking<'a>(&'a mut self, value: bool) -> impl Iterator<Item = cell::Position<'a>> {
		if value {
			self.mode.insert(mode::BLINK);
		}
		else {
			self.mode.remove(mode::BLINK);
		}

		self.iter(self.area.absolute()).filter(|c| c.style().attributes().contains(style::BLINK))
	}

	/// Handle a key.
	pub fn key<O: Write>(&mut self, key: Key, output: O) -> error::Result<impl Iterator<Item = (u32, u32)>> {
		if !self.mode.contains(mode::KEYBOARD_LOCK) {
			try!(key.write(self.mode, output));
		}

		Ok(iter::empty())
	}

	/// Handle output from the tty.
	pub fn handle<I: AsRef<[u8]>, O: Write>(&mut self, input: I, mut output: O) -> error::Result<(impl Iterator<Item = Action>, impl Iterator<Item = (u32, u32)>)> {
		// Juggle the incomplete buffer cache and the real input.
		let     input  = input.as_ref();
		let mut buffer = self.cache.take();

		if let Some(buffer) = buffer.as_mut() {
			buffer.extend_from_slice(input);
		}

		let     buffer  = buffer.as_ref();
		let mut input   = buffer.as_ref().map(AsRef::as_ref).unwrap_or(input);
		let mut actions = Vec::new(): Vec<Action>;

		loop {
			if input.is_empty() {
				break;
			}

			// Try to parse the rest of the input.
			let item = match control::parse(input) {
				// This should never happen.
				control::Result::Error(err) => {
					error!("cannot parse control code: {:?}", err);
					break;
				}

				// The given input isn't a complete sequence, cache the current input.
				control::Result::Incomplete(_) => {
					self.cache = Some(input.to_vec());
					break;
				}

				// We got a sequence or a raw input.
				control::Result::Done(rest, item) => {
					input = rest;
					item
				}
			};

			match item {
				// Handle custom DEC escape sequences.
				Control::C0(C0::Escape) => {
					if input.is_empty() {
						continue;
					}

					let code = input[0];
					input = &input[1..];

					match code {
						b'#' => {
							if input.is_empty() {
								continue;
							}

							let code = input[0];
							input = &input[1..];

							match code {
								// DECALN
								b'8' => {
									for y in 0 .. self.area.height {
										let row  = term!(self; row for y);

										for cell in &mut self.rows[row] {
											cell.into_occupied("E", self.cursor.style().clone());
										}
									}

									term!(self; touched all);
								}

								// DECBI
								b'6' => {
									if self.cursor.x() == 0 {
										let row = term!(self; row for 0);

										for y in row .. self.area.height as usize {
											let row = &mut self.rows[y];

											row.pop_back();
											row.push_front(Cell::empty(self.cursor.style().clone()));
										}
									}
									else {
										term!(self; cursor Left(1));
									}
								}

								// DECFI
								b'9' => {
									if self.cursor.x() == self.area.width - 1 {
										let row = term!(self; row for 0);

										for y in row .. self.area.height as usize {
											let row = &mut self.rows[y];

											row.pop_front();
											row.push_back(Cell::empty(self.cursor.style().clone()));
										}
									}
									else {
										term!(self; cursor Right(1));
									}
								}

								_ => {
									error!("unknown sequence: ESC # {:?}", code);
								}
							}
						}

						// DECSC
						b'7' => {
							self.saved = Some(self.cursor.clone());
						}

						// DECRC
						b'8' => {
							if let Some(cursor) = self.saved.take() {
								self.cursor = cursor;
							}
						}

						// DECKPAM
						b'=' => {
							self.mode.insert(mode::APPLICATION_KEYPAD);
						}

						// DECKPNM
						b'>' => {
							self.mode.remove(mode::APPLICATION_KEYPAD);
						}

						b'9' => {
							// TODO: move cursor forward one column, if it is at the margin,
							// drop the first column.
						}

						_ => {
							error!("unknown sequence: ESC {:?}", code);
						}
					}
				}

				// Attributes.
				Control::C1(C1::ControlSequence(CSI::DeviceAttributes(0))) => {
					try!(output.write_all(b"\033[?6c"));
				}

				Control::C1(C1::ControlSequence(CSI::Set(modes))) => {
					for mode in modes {
						match mode {
							CSI::Mode::KeyboardAction =>
								self.mode.insert(mode::KEYBOARD_LOCK),

							CSI::Mode::InsertionReplacement =>
								self.mode.insert(mode::INSERT),

							CSI::Mode::SendReceive =>
								self.mode.insert(mode::ECHO),

							CSI::Mode::LineFeed =>
								self.mode.insert(mode::CRLF),

							_ => ()
						}
					}
				}

				Control::C1(C1::ControlSequence(CSI::Private(b'h', None, args))) => {
					for arg in args {
						match arg {
							Some(1) =>
								self.mode.insert(mode::APPLICATION_CURSOR),

							Some(5) =>
								self.mode.insert(mode::REVERSE),

							Some(7) =>
								self.mode.insert(mode::WRAP),

							Some(25) =>
								self.cursor.visible = false,

							Some(1004) =>
								self.mode.insert(mode::FOCUS),

							Some(2004) =>
								self.mode.insert(mode::BRACKETED_PASTE),

							_ => (),
						}
					}
				}

				Control::C1(C1::ControlSequence(CSI::Reset(modes))) => {
					for mode in modes {
						match mode {
							CSI::Mode::KeyboardAction =>
								self.mode.remove(mode::KEYBOARD_LOCK),

							CSI::Mode::InsertionReplacement =>
								self.mode.remove(mode::INSERT),

							CSI::Mode::SendReceive =>
								self.mode.remove(mode::ECHO),

							CSI::Mode::LineFeed =>
								self.mode.remove(mode::CRLF),

							_ => ()
						}
					}
				}

				Control::C1(C1::ControlSequence(CSI::Private(b'l', None, args))) => {
					for arg in args {
						match arg {
							Some(1) =>
								self.mode.remove(mode::APPLICATION_CURSOR),

							Some(5) =>
								self.mode.remove(mode::REVERSE),

							Some(7) =>
								self.mode.remove(mode::WRAP),

							Some(25) =>
								self.cursor.visible = true,

							Some(1004) =>
								self.mode.remove(mode::FOCUS),

							Some(2004) =>
								self.mode.remove(mode::BRACKETED_PASTE),

							_ => (),
						}
					}
				}

				Control::C1(C1::ControlSequence(CSI::SaveCursor)) => {
					self.saved = Some(self.cursor.clone());
				}

				Control::C1(C1::ControlSequence(CSI::RestoreCursor)) => {
					if let Some(cursor) = self.saved.take() {
						self.cursor = cursor;
					}
				}

				// Movement functions.
				Control::C0(C0::CarriageReturn) => {
					term!(self; cursor Position(Some(0), None));
				}

				Control::C0(C0::LineFeed) => {
					term!(self; cursor Down(1));
				}

				Control::C0(C0::Backspace) => {
					term!(self; cursor Left(1));
				}

				Control::C1(C1::ControlSequence(CSI::CursorPosition { x, y })) => {
					term!(self; cursor Position(Some(x), Some(y)));
				}

				Control::C1(C1::ControlSequence(CSI::CursorUp(n))) => {
					term!(self; cursor Up(n));
				}

				Control::C1(C1::ControlSequence(CSI::CursorDown(n))) => {
					term!(self; cursor Down(n));
				}

				Control::C1(C1::ControlSequence(CSI::CursorBack(n))) => {
					term!(self; cursor Left(n));
				}

				Control::C1(C1::ControlSequence(CSI::CursorForward(n))) => {
					term!(self; cursor Right(n));
				}

				Control::C1(C1::ControlSequence(CSI::LinePosition(n))) => {
					term!(self; cursor Position(None, Some(n)));
				}

				// Erase functions.
				Control::C1(C1::ControlSequence(CSI::EraseDisplay(CSI::Erase::ToEnd))) => {
					let (x, y) = self.cursor.position();

					for x in x .. self.area.width {
						term!(self; touched (x, y));
						term!(self; mut cell x, y).into_empty(self.cursor.style().clone());
					}

					for y in y .. self.area.height {
						term!(self; touched line y);

						for x in 0 .. self.area.width {
							term!(self; mut cell x, y).into_empty(self.cursor.style().clone());
						}
					}
				}

				Control::C1(C1::ControlSequence(CSI::EraseDisplay(CSI::Erase::ToStart))) => {
					let (x, y) = self.cursor.position();

					for x in 0 .. x {
						term!(self; touched (x, y));
						term!(self; mut cell x, y).into_empty(self.cursor.style().clone());
					}

					for y in 0 .. y {
						term!(self; touched line y);

						for x in 0 .. self.area.width {
							term!(self; mut cell x, y).into_empty(self.cursor.style().clone());
						}
					}
				}

				Control::C1(C1::ControlSequence(CSI::EraseDisplay(CSI::Erase::All))) => {
					term!(self; touched all);

					for y in 0 .. self.area.height {
						for x in 0 .. self.area.width {
							term!(self; mut cell x, y).into_empty(self.cursor.style().clone());
						}
					}
				}

				Control::C1(C1::ControlSequence(CSI::EraseLine(CSI::Erase::ToEnd))) => {
					let (x, y) = self.cursor.position();

					for x in x .. self.area.width {
						term!(self; touched (x, y));
						term!(self; mut cell x, y).into_empty(self.cursor.style().clone());
					}
				}

				Control::C1(C1::ControlSequence(CSI::EraseLine(CSI::Erase::ToStart))) => {
					let (x, y) = self.cursor.position();

					for x in 0 .. x {
						term!(self; touched (x, y));
						term!(self; mut cell x, y).into_empty(self.cursor.style().clone());
					}
				}

				Control::C1(C1::ControlSequence(CSI::EraseLine(CSI::Erase::All))) => {
					let y = self.cursor.y();

					term!(self; touched line y);

					for x in 0 .. self.area.width {
						term!(self; mut cell x, y).into_empty(self.cursor.style().clone());
					}
				}

				Control::C1(C1::ControlSequence(CSI::DeleteLine(n))) => {
					let y   = self.cursor.y();
					let row = term!(self; row for y);

					// Remove the lines.
					self.rows.drain(row as usize .. (row + n as usize));

					// Fill missing lines.
					for _ in 0 .. n {
						term!(self; extend);
					}

					// Mark the affected lines as touched.
					for y in y .. self.area.height {
						term!(self; touched line y);
					}
				}

				// Insertion functions.
				Control::C1(C1::NextLine) => {
					term!(self; cursor Down(1));
					term!(self; cursor Position(Some(0), None));
				}

				Control::C1(C1::ControlSequence(CSI::InsertLine(n))) => {
					let y   = self.cursor.y();
					let row = term!(self; row for y);

					// Split the rows at the current line.
					let mut rest = self.rows.split_off(row);

					// Extend with new lines.
					for _ in 0 .. n {
						term!(self; extend);
					}

					// Remove the scrolled off lines.
					rest.drain((self.area.height - y - n) as usize ..);
					self.rows.append(&mut rest);

					// Mark the affected lines as touched.
					for y in y .. self.area.height {
						term!(self; touched line y);
					}
				}

				Control::None(string) => {
					for ch in string.graphemes(true) {
						let width = ch.width() as u32;

						// If the character overflows the area, wrap it down.
						if self.cursor.x() + width > self.area.width {
							term!(self; cursor Down(1));
							term!(self; cursor Position(Some(1), None));
						}

						// Change the cells appropriately.
						{
							let x   = self.cursor.x();
							let y   = self.cursor.y();
							let row = term!(self; row for y);
							let row = &mut self.rows[row];

							let changed = match row[x as usize] {
								Cell::Empty { .. } =>
									true,

								Cell::Occupied { ref style, ref value, .. } =>
									value != ch || style != self.cursor.style(),

								Cell::Reference(..) =>
									false
							};

							if changed {
								row[x as usize].into_occupied(ch, self.cursor.style().clone());
								term!(self; touched (x, y));

								for (i, x) in (x + 1 .. x + width).enumerate() {
									row[x as usize].into_reference(i as u8 + 1);
								}
							}
						}

						term!(self; cursor Right(width));
					}
				}

				// Style functions.
				Control::C1(C1::ControlSequence(CSI::SelectGraphicalRendition(attrs))) => {
					let mut style = **self.cursor.style();

					for attr in &attrs {
						match *attr {
							SGR::Reset =>
								style = Style::default(),

							SGR::Font(SGR::Weight::Normal) =>
								style.attributes.remove(style::BOLD | style::FAINT),

							SGR::Font(SGR::Weight::Bold) => {
								style.attributes.remove(style::FAINT);
								style.attributes.insert(style::BOLD);
							}

							SGR::Font(SGR::Weight::Faint) => {
								style.attributes.remove(style::BOLD);
								style.attributes.insert(style::FAINT);
							}

							SGR::Italic(true) =>
								style.attributes.insert(style::ITALIC),
							SGR::Italic(false) =>
								style.attributes.remove(style::ITALIC),

							SGR::Underline(true) =>
								style.attributes.insert(style::UNDERLINE),
							SGR::Underline(false) =>
								style.attributes.remove(style::UNDERLINE),

							SGR::Blink(true) =>
								style.attributes.insert(style::BLINK),
							SGR::Blink(false) =>
								style.attributes.remove(style::BLINK),

							SGR::Reverse(true) =>
								style.attributes.insert(style::REVERSE),
							SGR::Reverse(false) =>
								style.attributes.remove(style::REVERSE),

							SGR::Invisible(true) =>
								style.attributes.insert(style::INVISIBLE),
							SGR::Invisible(false) =>
								style.attributes.remove(style::INVISIBLE),

							SGR::Struck(true) =>
								style.attributes.insert(style::STRUCK),
							SGR::Struck(false) =>
								style.attributes.remove(style::STRUCK),

							SGR::Foreground(ref color) =>
								style.foreground = Some(to_rgba(color, &self.config, true)),

							SGR::Background(ref color) =>
								style.background = Some(to_rgba(color, &self.config, false)),
						}
					}

					self.cursor.update(style);
				}

				// DECSCUSR
				Control::C1(C1::ControlSequence(CSI::Unknown(b'q', Some(b' '), args))) => {
					match arg!(args[0] => 0) {
						0 | 1 => {
							self.cursor.blink = true;
							self.cursor.shape = Shape::Block;
						}

						2 => {
							self.cursor.blink = false;
							self.cursor.shape = Shape::Block;
						}

						3 => {
							self.cursor.blink = true;
							self.cursor.shape = Shape::Line;
						}

						4 => {
							self.cursor.blink = false;
							self.cursor.shape = Shape::Line;
						}

						5 => {
							self.cursor.blink = true;
							self.cursor.shape = Shape::Beam;
						}

						6 => {
							self.cursor.blink = false;
							self.cursor.shape = Shape::Beam;
						}

						_ => ()
					}
				}

				Control::C1(C1::OperatingSystemCommand(cmd)) if cmd.starts_with("cursor:") => {
					let mut parts = cmd.split(':').skip(1);

					match parts.next() {
						Some("show") => {
							self.cursor.visible = true;
						}

						Some("hide") => {
							self.cursor.visible = false;
						}

						Some("background") => {
							let     desc  = parts.next().unwrap_or("-");
							let mut color = *self.config.style().cursor().background();

							if let Some(c) = config::to_color(desc) {
								color = c;
							}

							self.cursor.background = color;
						}

						Some(_) | None => ()
					}
				}

				Control::C1(C1::OperatingSystemCommand(cmd)) if cmd.starts_with("0;") || cmd.starts_with("k;") => {
					actions.push(Action::Title(String::from(&cmd[2..])));
				}

				code => {
					debug!("unhandled control code: {:?}", code);
				}
			}
		}

		Ok((actions.into_iter(), self.touched.iter(self.area)))
	}
}

fn to_rgba(color: &SGR::Color, config: &Config, foreground: bool) -> Rgba<f64> {
	match *color {
		SGR::Color::Default => {
			if foreground {
				*config.style().color().foreground()
			}
			else {
				*config.style().color().background()
			}
		}

		SGR::Color::Transparent =>
			Rgba::new(0.0, 0.0, 0.0, 0.0),

		SGR::Color::Index(index) =>
			*config.color().get(index),

		SGR::Color::Rgb(r, g, b) =>
			Rgba::new_u8(r, g, b, 255),

		SGR::Color::Cmy(..) |
		SGR::Color::Cmyk(..) =>
			unreachable!(),
	}
}
