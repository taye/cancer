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

use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use fnv::FnvHasher;
use std::f32;

use picto::color::{Rgba, Hsl, RgbHue};
use control::DEC::SIXEL;
use sys::cairo;

#[derive(Debug)]
pub struct Sixel {
	raster: SIXEL::Header,

	grid:   Vec<Vec<cairo::Image>>,
	width:  u32,
	height: u32,
	x:      u32,
	y:      u32,

	colors:     HashMap<u32, (u8, u8, u8, u8), BuildHasherDefault<FnvHasher>>,
	color:      u32,
	background: (u8, u8, u8, u8),
}

impl Sixel {
	pub fn new(header: SIXEL::Header, background: &Rgba<f64>, width: u32, height: u32) -> Self {
		Sixel {
			raster: header,

			grid:   Default::default(),
			width:  width,
			height: height,
			x:      0,
			y:      0,

			colors:     Default::default(),
			color:      0,
			background: (
				(background.red   * 255.0) as u8,
				(background.green * 255.0) as u8,
				(background.blue  * 255.0) as u8,
				(background.alpha * 255.0) as u8),
		}
	}

	pub fn rows(&self) -> usize {
		self.grid.len()
	}

	pub fn into_inner(self) -> Vec<Vec<cairo::Image>> {
		self.grid
	}

	pub fn aspect(&mut self, aspect: (u32, u32)) {
		self.raster.aspect = aspect;
	}

	pub fn enable(&mut self, id: u32) {
		self.color = id;
	}

	pub fn define(&mut self, id: u32, color: SIXEL::Color) {
		let color = match color {
			SIXEL::Color::Hsl(h, s, l) =>
				Rgba::from(Hsl::new(RgbHue::from_radians(h as f32 * f32::consts::PI / 180.0),
					s as f32 / 100.0, l as f32 / 100.0)).to_pixel(),

			SIXEL::Color::Rgb(r, g, b) =>
				(r, g, b, 255),

			SIXEL::Color::Rgba(r, g, b, a) =>
				(r, g, b, a),
		};

		self.colors.insert(id, color);
	}

	pub fn start(&mut self) {
		self.x = 0;
	}

	pub fn next(&mut self) {
		self.x  = 0;
		self.y += 6 * self.raster.aspect.0;
	}

	pub fn draw(&mut self, value: SIXEL::Map) {
		let color = self.colors.get(&self.color).unwrap_or(&self.background);

		let x  = (self.x / self.width) as usize;
		let xo = self.x % self.width;

		for (i, y) in (self.y .. self.y + (6 * self.raster.aspect.0)).enumerate() {
			let i  = (i as u32 / self.raster.aspect.0) as u8;
			let yo = y as u32 % self.height;
			let y  = (y / self.height) as usize;

			if y >= self.grid.len() {
				self.grid.push(Vec::new());
			}

			if x >= self.grid[y].len() {
				self.grid[y].push(cairo::Image::new(self.width, self.height));
			}

			if value.get(i) {
				self.grid[y][x].set(xo, yo, *color);
			}
			else if self.raster.background {
				self.grid[y][x].set(xo, yo, self.background);
			}
		}

		self.x += 1;
	}
}
