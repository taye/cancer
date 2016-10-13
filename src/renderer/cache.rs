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

use lru::LruCache;
use std::hash::BuildHasherDefault;
use fnv::FnvHasher;

use sys::pango;
use style::{self, Style};
use terminal::cell;
use font::Font;

#[derive(Debug)]
pub struct Cache {
	width:  u32,
	height: u32,
	inner:  Vec<Cell>,

	font:   Arc<Font>,
	glyphs: LruCache<Glyph, Computed, BuildHasherDefault<FnvHasher>>,
}

#[derive(Clone, Default, Debug)]
pub struct Cell {
	style: Rc<Style>,
	value: Option<String>,
	valid: bool,
}

#[derive(Clone, Debug)]
pub struct Computed {
	font:  pango::Font,
	shape: pango::GlyphString,
}

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct Glyph {
	value: String,
	attrs: style::Attributes,
}

impl Glyph {
	pub fn new<T: Into<String>>(value: T, attrs: style::Attributes) -> Self {
		Glyph {
			value: value.into(),
			attrs: attrs & (style::BOLD | style::FAINT | style::ITALIC),
		}
	}
}

impl Cache {
	/// Create a new cache of the given size.
	pub fn new(capacity: usize, font: Arc<Font>, width: u32, height: u32) -> Self {
		let style = Rc::new(Style::default());

		Cache {
			width:  width,
			height: height,
			inner:  vec![Cell::empty(style.clone()); (width * height) as usize],

			font:   font,
			glyphs: LruCache::with_hasher(capacity, Default::default()),
		}
	}

	/// Resize the cache, and invalidate it.
	pub fn resize(&mut self, width: u32, height: u32) {
		self.width  = width;
		self.height = height;

		let style  = Rc::new(Style::default());
		self.inner = vec![Cell::empty(style.clone()); (width * height) as usize];
	}

	/// Invalidate the given cell.
	pub fn invalidate(&mut self, cell: &cell::Position) {
		debug_assert!(!cell.is_reference());

		self.inner[(cell.y() * self.width + cell.x()) as usize].valid = false;
	}

	/// Update the cache, returns `false` if nothing was changed.
	pub fn update(&mut self, cell: &cell::Position) -> bool {
		debug_assert!(!cell.is_reference());

		let cache = &mut self.inner[(cell.y() * self.width + cell.x()) as usize];

		// Check if the cache is up to date.
		if cache.valid &&
		   cell.style() == &cache.style &&
		   ((cell.is_empty() && cache.value.is_none()) ||
		    (cell.is_occupied() && cache.value.as_ref().map(AsRef::as_ref) == Some(cell.value())))
		{
			return false;
		}

		*cache = Cell {
			style: cell.style().clone(),
			value: if cell.is_empty() { None } else { Some(cell.value().into()) },
			valid: true,
		};

		true
	}

	pub fn compute<T: AsRef<str>>(&mut self, string: T, attrs: style::Attributes) -> Computed {
		let glyph = Glyph::new(string.as_ref(), attrs);

		if let Some(computed) = self.glyphs.get_mut(&glyph) {
			return computed.clone();
		}

		let computed = Computed {
			font:  self.font.font(glyph.value.chars().next().unwrap(), glyph.attrs),
			shape: self.font.shape(&glyph.value, glyph.attrs),
		};

		self.glyphs.insert(glyph, computed.clone());
		computed
	}
}

impl Cell {
	pub fn empty(style: Rc<Style>) -> Self {
		Cell {
			style: style,
			value: None,
			valid: true,
		}
	}
}

impl Computed {
	pub fn font(&self) -> &pango::Font {
		&self.font
	}

	pub fn shape(&self) -> &pango::GlyphString {
		&self.shape
	}
}