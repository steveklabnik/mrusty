// mrusty. mruby bindings for Rust
// Copyright (C) 2016  Dragoș Tiselice
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use std::cell::RefCell;
use std::rc::Rc;

use mrusty::*;

use api::Scalar;

#[derive(Clone, Debug, PartialEq)]
pub struct Vector {
    pub x: f32,
    pub y: f32,
    pub z: f32
}

impl Vector {
    pub fn new(x: f32, y: f32, z: f32) -> Vector {
        Vector {
            x: x,
            y: y,
            z: z
        }
    }

    pub fn to_mruby(mruby: Rc<RefCell<MRuby>>) {
        mruby.def_class::<Vector>("Vector");

        mruby.def_method::<Vector, _>("initialize", mrfn!(|mruby, slf, x: f64, y: f64, z: f64| {
            let vector = Vector::new(x as f32, y as f32, z as f32);

            slf.init(vector)
        }));

        mruby.def_method::<Vector, _>("x", mrfn!(|mruby, slf| {
            mruby.float(slf.to_obj::<Vector>().unwrap().x as f64)
        }));

        mruby.def_method::<Vector, _>("y", mrfn!(|mruby, slf| {
            mruby.float(slf.to_obj::<Vector>().unwrap().y as f64)
        }));

        mruby.def_method::<Vector, _>("z", mrfn!(|mruby, slf| {
            mruby.float(slf.to_obj::<Vector>().unwrap().z as f64)
        }));

        mruby.def_method::<Vector, _>("to_a", mrfn!(|mruby, slf| {
            let vector = slf.to_obj::<Vector>().unwrap();

            mruby.array(vec![
                mruby.float(vector.x as f64),
                mruby.float(vector.y as f64),
                mruby.float(vector.z as f64)
            ])
        }));
    }
}

use std::ops::Mul;

impl Mul<Vector> for Scalar {
    type Output = Vector;

    fn mul(self, vector: Vector) -> Vector {
        Vector {
            x: vector.x * self.value,
            y: vector.y * self.value,
            z: vector.z * self.value
        }
    }
}