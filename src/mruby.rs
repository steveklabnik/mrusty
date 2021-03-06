// mrusty. mruby safe bindings for Rust
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

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{self, Read};
use std::mem;
use std::os::raw::{c_char, c_void};
use std::panic::{self, AssertRecoverSafe};
use std::path::Path;
use std::rc::Rc;

use super::mruby_ffi::*;

/// A `type` wrapper around a `Rc<RefCell<Mruby>>`. Created with `Mruby::new()`.
pub type MrubyType = Rc<RefCell<Mruby>>;

/// A safe `struct` for the mruby API. The `struct` only contains creation and desctruction
/// methods. Creating an `Mruby` returns a `MrubyType` (`Rc<RefCell<Mruby>>`) which implements
/// `MrubyImpl` where the rest of the implemented API is found.
///
/// # Examples
///
/// ```
/// use mrusty::*;
///
/// let mruby = Mruby::new();
/// let result = mruby.run("2 + 2 == 5").unwrap();
///
/// assert_eq!(result.to_bool().unwrap(), false);
/// ```
pub struct Mruby {
    pub mrb:       *const MrState,
    ctx:           *const MrContext,
    filename:      Option<String>,
    classes:       HashMap<TypeId, (*const MrClass, MrDataType, String)>,
    methods:       HashMap<TypeId, HashMap<u32, Rc<Fn(MrubyType, Value) -> Value>>>,
    class_methods: HashMap<TypeId, HashMap<u32, Rc<Fn(MrubyType, Value) -> Value>>>,
    files:         HashMap<String, Vec<fn(MrubyType)>>,
    required:      HashSet<String>
}

impl Mruby {
    /// Creates an mruby state and context stored in a `MrubyType` (`Rc<RefCell<Mruby>>`).
    ///
    /// # Example
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// let mruby = Mruby::new();
    /// ```
    pub fn new() -> MrubyType {
        unsafe {
            let mrb = mrb_open();

            let mruby = Rc::new(RefCell::new(
                Mruby {
                    mrb:           mrb,
                    ctx:           mrbc_context_new(mrb),
                    filename:      None,
                    classes:       HashMap::new(),
                    methods:       HashMap::new(),
                    class_methods: HashMap::new(),
                    files:         HashMap::new(),
                    required:      HashSet::new()
                }
            ));

            let kernel = mrb_module_get(mrb, CString::new("Kernel").unwrap().as_ptr());

            extern "C" fn require(mrb: *const MrState, _slf: MrValue) -> MrValue {
                unsafe {
                    let ptr = mrb_ext_get_ud(mrb);
                    let mruby = mem::transmute::<*const u8, MrubyType>(ptr);

                    let name = mem::uninitialized::<*const c_char>();

                    mrb_get_args(mrb, CString::new("z").unwrap().as_ptr(),
                                 &name as *const *const c_char);

                    let name = CStr::from_ptr(name).to_str().unwrap();

                    let already_required = {
                        mruby.borrow().required.contains(name)
                    };

                    let result = if already_required {
                        mruby.bool(false)
                    } else {
                        let reqs = {
                            let borrow = mruby.borrow();

                            borrow.files.get(name).map(|reqs| reqs.clone())
                        };

                        match reqs {
                            Some(reqs) => {
                                { mruby.borrow_mut().required.insert(name.to_owned()); }

                                for req in reqs {
                                    req(mruby.clone());
                                }

                                mruby.bool(true)
                            },
                            None => {
                                let filename = mruby.borrow().filename.clone();

                                let execute = |path: &Path, name: String,
                                               filename: Option<String>| {
                                    { mruby.borrow_mut().required.insert(name); }

                                    let result = mruby.execute(path);

                                    match filename {
                                        Some(filename) => mruby.filename(&filename),
                                        None           => mruby.borrow_mut().filename = None
                                    }

                                    match result {
                                        Err(err) => {
                                            mruby.raise("RuntimeError", &format!("{}", err));
                                        }
                                        _ => ()
                                    }

                                    mruby.bool(true)
                                };

                                let path = Path::new(name);
                                let rb = name.to_owned() + ".rb";
                                let rb = Path::new(&rb);
                                let mrb = name.to_owned() + ".mrb";
                                let mrb = Path::new(&mrb);

                                if rb.is_file() {
                                    execute(rb, name.to_owned(), filename)
                                } else if mrb.is_file() {
                                    execute(mrb, name.to_owned(), filename)
                                } else if path.is_file() {
                                    execute(path, name.to_owned(), filename)
                                } else {
                                    mruby.raise("RuntimeError",
                                                &format!("cannot load {}.rb or {}.mrb",
                                                         name, name))
                                }
                            }
                        }
                    };

                    mem::forget(mruby);

                    result.value
                }
            }

            mrb_define_module_function(mrb, kernel, CString::new("require").unwrap().as_ptr(),
                                       require, 1 << 12);

            let ptr = mem::transmute::<MrubyType, *const u8>(mruby);
            mrb_ext_set_ud(mrb, ptr);

            let mruby = mem::transmute::<*const u8, MrubyType>(ptr);

            mruby.run_unchecked("
              class RustPanic < Exception
                def initialize(message)
                  super message
                end
              end
            ");

            mruby
        }
    }

    fn close(&self) {
        unsafe {
            mrb_close(self.mrb);
        }
    }
}

/// An `enum` containing all possbile types of errors.
#[derive(Debug)]
pub enum MrubyError {
    /// type cast error
    Cast(String),
    /// undefined type error
    Undef,
    /// mruby runtime error
    Runtime(String),
    /// unrecognized file type error
    Filetype,
    /// Rust `Io` error
    Io(io::Error)
}

impl fmt::Display for MrubyError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            MrubyError::Cast(ref expected) => {
                write!(f, "Cast error: expected {}", expected)
            },
            MrubyError::Undef => {
                write!(f, "Undefined error: type is not defined")
            },
            MrubyError::Runtime(ref err) => {
                write!(f, "Runtime error: {}", err)
            },
            MrubyError::Filetype => {
                write!(f, "Filetype error: script needs a compatible (.rb, .mrb) extension")
            },
            MrubyError::Io(ref err) => err.fmt(f)
        }
    }
}

impl Error for MrubyError {
    fn description(&self) -> &str {
        match *self {
            MrubyError::Cast(_)     => "mruby value cast error",
            MrubyError::Undef       => "mruby undefined error",
            MrubyError::Runtime(_)  => "mruby runtime error",
            MrubyError::Filetype    => "filetype mistmatch",
            MrubyError::Io(ref err) => err.description()
        }
    }
}

impl From<io::Error> for MrubyError {
    fn from(err: io::Error) -> MrubyError {
        MrubyError::Io(err)
    }
}

/// A `trait` useful for organising Rust types into dynamic mruby files.
///
/// # Examples
///
/// ```
/// # use mrusty::Mruby;
/// # use mrusty::MrubyFile;
/// # use mrusty::MrubyImpl;
/// # use mrusty::MrubyType;
/// struct Cont {
///     value: i32
/// }
///
/// impl MrubyFile for Cont {
///     fn require(mruby: MrubyType) {
///         mruby.def_class::<Cont>("Container");
///     }
/// }
///
/// let mruby = Mruby::new();
///
/// mruby.def_file::<Cont>("cont");
/// ```
pub trait MrubyFile {
    fn require(mruby: MrubyType);
}

/// A `trait` used on `MrubyType` which implements mruby functionality.
pub trait MrubyImpl {
    /// Adds a filename to the mruby context.
    ///
    /// # Examples
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyError;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    /// mruby.filename("script.rb");
    ///
    /// let result = mruby.run("1.nope");
    ///
    /// match result {
    ///     Err(MrubyError::Runtime(err)) => {
    ///         assert_eq!(err, "script.rb:1: undefined method \'nope\' for 1 (NoMethodError)");
    /// },
    ///     _ => assert!(false)
    /// }
    /// ```
    #[inline]
    fn filename(&self, filename: &str);

    /// Runs mruby `script` on a state and context and returns a `Value` in an `Ok`
    /// or an `Err` containing an mruby `Exception`'s message.
    ///
    /// # Examples
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    /// let result = mruby.run("true").unwrap();
    ///
    /// assert_eq!(result.to_bool().unwrap(), true);
    /// ```
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyError;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    /// let result = mruby.run("'' + 1");
    ///
    /// match result {
    ///     Err(MrubyError::Runtime(err)) => {
    ///         assert_eq!(err, "TypeError: expected String");
    /// },
    ///     _ => assert!(false)
    /// }
    /// ```
    #[inline]
    fn run(&self, script: &str) -> Result<Value, MrubyError>;

    /// Runs mruby `script` on a state and context and returns a `Value`. If an mruby Exception is
    /// raised, mruby will be left to handle it.
    /// # Examples
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    /// let result = mruby.run_unchecked("true");
    ///
    /// assert_eq!(result.to_bool().unwrap(), true);
    /// ```
    ///
    /// ```
    /// # #[macro_use] extern crate mrusty;
    /// use mrusty::*;
    ///
    /// # fn main() {
    /// let mruby = Mruby::new();
    ///
    /// struct Cont;
    ///
    /// mruby.def_class::<Cont>("Container");
    /// mruby.def_class_method::<Cont, _>("raise", mrfn!(|mruby, _slf: Value| {
    ///     mruby.run_unchecked("fail 'surprize'")
    /// }));
    ///
    /// let result = mruby.run("
    ///   begin
    ///     Container.raise
    ///   rescue => e
    ///     e.message
    ///   end
    /// ").unwrap();
    ///
    /// assert_eq!(result.to_str().unwrap(), "surprize");
    /// # }
    /// ```
    #[inline]
    fn run_unchecked(&self, script: &str) -> Value;

    /// Runs mruby compiled (.mrb) `script` on a state and context and returns a `Value` in an `Ok`
    /// or an `Err` containing an mruby `Exception`'s message.
    ///
    /// # Examples
    ///
    /// ```no-run
    /// let mruby = Mruby::new();
    /// let result = mruby.runb(include_bytes!("script.mrb")).unwrap();
    /// ```
    #[inline]
    fn runb(&self, script: &[u8]) -> Result<Value, MrubyError>;

    /// Runs mruby (compiled (.mrb) or not (.rb)) `script` on a state and context and returns a
    /// `Value` in an `Ok` or an `Err` containing an mruby `Exception`'s message.
    ///
    /// # Examples
    ///
    /// ```no-run
    /// let mruby = Mruby::new();
    /// let result = mruby.execute(File::open("script.rb")).unwrap();
    /// ```
    #[inline]
    fn execute(&self, script: &Path) -> Result<Value, MrubyError>;

    /// Raises an mruby `RuntimeError` with `message` message and `eclass` mruby Exception Class.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate mrusty;
    /// use mrusty::*;
    ///
    /// # fn main() {
    /// let mruby = Mruby::new();
    ///
    /// struct Cont;
    ///
    /// mruby.def_class::<Cont>("Container");
    /// mruby.def_class_method::<Cont, _>("hi", mrfn!(|mruby, _slf: Value| {
    ///     mruby.raise("RuntimeError", "hi");
    ///
    ///     mruby.nil()
    /// }));
    ///
    /// let result = mruby.run("Container.hi");
    ///
    /// match result {
    ///     Err(MrubyError::Runtime(err)) => {
    ///         assert_eq!(err, "RuntimeError: hi");
    /// },
    ///     _ => assert!(false)
    /// }
    /// # }
    /// ```
    #[inline]
    fn raise(&self, eclass: &str, message: &str) -> Value;

    /// Defines a dynamic file that can be `require`d containing the Rust type `T` and runs its
    /// `MrubyFile`-inherited `require` method.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate mrusty;
    /// use mrusty::*;
    ///
    /// # fn main() {
    /// let mruby = Mruby::new();
    ///
    /// struct Cont {
    ///     value: i32
    /// };
    ///
    /// impl MrubyFile for Cont {
    ///     fn require(mruby: MrubyType) {
    ///         mruby.def_class::<Cont>("Container");
    ///         mruby.def_method::<Cont, _>("initialize", mrfn!(|_mruby, slf: Value, v: i32| {
    ///             let cont = Cont { value: v };
    ///
    ///             slf.init(cont)
    ///         }));
    ///         mruby.def_method::<Cont, _>("value", mrfn!(|mruby, slf: Cont| {
    ///             mruby.fixnum(slf.value)
    ///         }));
    ///     }
    /// }
    ///
    /// mruby.def_file::<Cont>("cont");
    ///
    /// let result = mruby.run("
    ///     require 'cont'
    ///
    ///     Container.new(3).value
    /// ").unwrap();
    ///
    /// assert_eq!(result.to_i32().unwrap(), 3);
    /// # }
    /// ```
    #[inline]
    fn def_file<T: MrubyFile>(&self, name: &str);

    /// Defines Rust type `T` as an mruby `Class` named `name`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// struct Cont {
    ///     value: i32
    /// }
    ///
    /// mruby.def_class::<Cont>("Container");
    /// ```
    fn def_class<T: Any>(&self, name: &str);

    /// Defines an mruby method named `name`. The closure to be run when the `name` method is
    /// called should be passed through the `mrfn!` macro.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate mrusty;
    /// use mrusty::*;
    ///
    /// # fn main() {
    /// let mruby = Mruby::new();
    ///
    /// struct Cont {
    ///     value: i32
    /// };
    ///
    /// mruby.def_class::<Cont>("Container");
    /// mruby.def_method::<Cont, _>("initialize", mrfn!(|_mruby, slf: Value, v: i32| {
    ///     let cont = Cont { value: v };
    ///
    ///     slf.init(cont)
    /// }));
    /// mruby.def_method::<Cont, _>("value", mrfn!(|mruby, slf: Cont| {
    ///     mruby.fixnum(slf.value)
    /// }));
    ///
    /// let result = mruby.run("Container.new(3).value").unwrap();
    ///
    /// assert_eq!(result.to_i32().unwrap(), 3);
    /// # }
    /// ```
    fn def_method<T: Any, F>(&self, name: &str,
                             method: F) where F: Fn(MrubyType, Value) -> Value + 'static;

    /// Defines an mruby class method named `name`. The closure to be run when the `name` method is
    /// called should be passed through the `mrfn!` macro.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate mrusty;
    /// use mrusty::*;
    ///
    /// # fn main() {
    /// let mruby = Mruby::new();
    ///
    /// struct Cont;
    ///
    /// mruby.def_class::<Cont>("Container");
    /// mruby.def_class_method::<Cont, _>("hi", mrfn!(|mruby, _slf: Value, v: i32| {
    ///     mruby.fixnum(v)
    /// }));
    ///
    /// let result = mruby.run("Container.hi 3").unwrap();
    ///
    /// assert_eq!(result.to_i32().unwrap(), 3);
    /// # }
    /// ```
    fn def_class_method<T: Any, F>(&self, name: &str,
                                   method: F) where F: Fn(MrubyType, Value) -> Value + 'static;

    /// Return the mruby name of a previously defined Rust type `T` with `def_class`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use mrusty::*;
    ///
    /// let mruby = Mruby::new();
    ///
    /// struct Cont;
    ///
    /// mruby.def_class::<Cont>("Container");
    ///
    /// assert_eq!(mruby.class_name::<Cont>().unwrap(), "Container");
    /// ```
    fn class_name<T: Any>(&self) -> Result<String, MrubyError>;

    /// Creates mruby `Value` `nil`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// struct Cont;
    ///
    /// mruby.def_class::<Cont>("Container");
    /// mruby.def_method::<Cont, _>("nil", |mruby, _slf| mruby.nil());
    ///
    /// let result = mruby.run("Container.new.nil.nil?").unwrap();
    ///
    /// assert_eq!(result.to_bool().unwrap(), true);
    /// ```
    #[inline]
    fn nil(&self) -> Value;

    /// Creates mruby `Value` containing `true` or `false`.
    ///
    /// # Examples
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// let b = mruby.bool(true);
    ///
    /// assert_eq!(b.to_bool().unwrap(), true);
    /// ```
    #[inline]
    fn bool(&self, value: bool) -> Value;

    /// Creates mruby `Value` of `Class` `Fixnum`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// let fixn = mruby.fixnum(2);
    ///
    /// assert_eq!(fixn.to_i32().unwrap(), 2);
    /// ```
    #[inline]
    fn fixnum(&self, value: i32) -> Value;

    /// Creates mruby `Value` of `Class` `Float`.
    ///
    /// # Examples
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// let fl = mruby.float(2.3);
    ///
    /// assert_eq!(fl.to_f64().unwrap(), 2.3);
    /// ```
    #[inline]
    fn float(&self, value: f64) -> Value;

    /// Creates mruby `Value` of `Class` `String`.
    ///
    /// # Examples
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// let s = mruby.string("hi");
    ///
    /// assert_eq!(s.to_str().unwrap(), "hi");
    /// ```
    #[inline]
    fn string(&self, value: &str) -> Value;

    /// Creates mruby `Value` of `Class` `Symbol`.
    ///
    /// # Examples
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// let s = mruby.symbol("hi");
    ///
    /// assert_eq!(s.to_str().unwrap(), "hi");
    /// ```
    #[inline]
    fn symbol(&self, value: &str) -> Value;

    /// Creates mruby `Value` of `Class` `name` containing a Rust object of type `T`.
    ///
    /// *Note:* `T` must be defined on the current `Mruby` with `def_class`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// struct Cont {
    ///     value: i32
    /// }
    ///
    /// mruby.def_class::<Cont>("Container");
    ///
    /// let value = mruby.obj(Cont { value: 3 });
    /// ```
    #[inline]
    fn obj<T: Any>(&self, obj: T) -> Value;

    /// Creates mruby `Value` of `Class` `name` containing a Rust `Option` of type `T`.
    ///
    /// *Note:* `T` must be defined on the current `Mruby` with `def_class`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// struct Cont {
    ///     value: i32
    /// }
    ///
    /// mruby.def_class::<Cont>("Container");
    ///
    /// let none = mruby.option::<Cont>(None);
    /// let some = mruby.option(Some(Cont { value: 3 }));
    ///
    /// assert_eq!(none.call("nil?", vec![]).unwrap().to_bool().unwrap(), true);
    /// assert_eq!(some.to_obj::<Cont>().unwrap().value, 3);
    /// ```
    #[inline]
    fn option<T: Any>(&self, obj: Option<T>) -> Value;

    /// Creates mruby `Value` of `Class` `Array`.
    ///
    /// # Examples
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// let array = mruby.array(vec![
    ///     mruby.fixnum(1),
    ///     mruby.fixnum(2),
    ///     mruby.fixnum(3)
    /// ]);
    ///
    /// assert_eq!(array.to_vec().unwrap(), vec![
    ///     mruby.fixnum(1),
    ///     mruby.fixnum(2),
    ///     mruby.fixnum(3)
    /// ]);
    /// ```
    #[inline]
    fn array(&self, value: Vec<Value>) -> Value;
}

impl MrubyImpl for MrubyType {
    #[inline]
    fn filename(&self, filename: &str) {
        self.borrow_mut().filename = Some(filename.to_owned());

        unsafe {
            mrbc_filename(self.borrow().mrb, self.borrow().ctx,
                          CString::new(filename).unwrap().as_ptr());
        }
    }

    #[inline]
    fn run(&self, script: &str) -> Result<Value, MrubyError> {
        unsafe {
            let (mrb, ctx) = {
                let borrow = self.borrow();

                (borrow.mrb, borrow.ctx)
            };

            let value = mrb_load_nstring_cxt(mrb, script.as_ptr(), script.len() as i32, ctx);
            let exc = mrb_ext_get_exc(self.borrow().mrb);

            match exc.typ {
                MrType::MRB_TT_FALSE => {
                    Ok(Value::new(self.clone(), value))
                },
                _ => Err(MrubyError::Runtime(exc.to_str(self.borrow().mrb).unwrap().to_owned()))
            }
        }
    }

    #[inline]
    fn run_unchecked(&self, script: &str) -> Value {
        unsafe {
            let (mrb, ctx) = {
                let borrow = self.borrow();

                (borrow.mrb, borrow.ctx)
            };

            let value = mrb_load_nstring_cxt(mrb, script.as_ptr(), script.len() as i32, ctx);

            Value::new(self.clone(), value)
        }
    }

    #[inline]
    fn runb(&self, script: &[u8]) -> Result<Value, MrubyError> {
        unsafe {
            let (mrb, ctx) = {
                let borrow = self.borrow();

                (borrow.mrb, borrow.ctx)
            };

            let value = mrb_load_irep_cxt(mrb, script.as_ptr(), ctx);
            let exc = mrb_ext_get_exc(self.borrow().mrb);

            match exc.typ {
                MrType::MRB_TT_FALSE => {
                    Ok(Value::new(self.clone(), value))
                },
                _ => Err(MrubyError::Runtime(exc.to_str(self.borrow().mrb).unwrap().to_owned()))
            }
        }
    }

    #[inline]
    fn execute(&self, script: &Path) -> Result<Value, MrubyError> {
        match script.extension() {
            Some(ext) => {
                self.filename(script.file_name().unwrap().to_str().unwrap());

                let mut file = try!(File::open(script));

                match ext.to_str().unwrap() {
                    "rb" => {
                        let mut script = String::new();
                        try!(file.read_to_string(&mut script));

                        self.run(&script)
                    },
                    "mrb" => {
                        let mut script = Vec::new();
                        try!(file.read_to_end(&mut script));

                        self.runb(&script)
                    },
                    _ => {
                        Err(MrubyError::Filetype)
                    }
                }
            },
            None => Err(MrubyError::Filetype)
        }
    }

    #[inline]
    fn raise(&self, eclass: &str, message: &str) -> Value {
        unsafe {
            mrb_ext_raise(self.borrow().mrb, CString::new(eclass).unwrap().as_ptr(),
                          CString::new(message).unwrap().as_ptr());

            self.nil()
        }
    }

    #[inline]
    fn def_file<T: MrubyFile>(&self, name: &str) {
        let mut borrow = self.borrow_mut();

        if borrow.files.contains_key(name) {
            let mut file = borrow.files.get_mut(name).unwrap();

            file.push(T::require);
        } else {
            borrow.files.insert(name.to_owned(), vec![T::require]);
        }
    }

    fn def_class<T: Any>(&self, name: &str) {
        unsafe {
            let name = name.to_owned();

            let c_name = CString::new(name.clone()).unwrap();
            let object = CString::new("Object").unwrap();
            let object = mrb_class_get(self.borrow().mrb, object.as_ptr());

            let class = mrb_define_class(self.borrow().mrb, c_name.as_ptr(), object);

            mrb_ext_set_instance_tt(class, MrType::MRB_TT_DATA);

            extern "C" fn free<T>(_mrb: *const MrState, ptr: *const u8) {
                unsafe {
                    mem::transmute::<*const u8, Rc<T>>(ptr);
                }
            }

            let data_type = MrDataType { name: c_name.as_ptr(), free: free::<T> };

            self.borrow_mut().classes.insert(TypeId::of::<T>(), (class, data_type, name));
            self.borrow_mut().methods.insert(TypeId::of::<T>(), HashMap::new());
            self.borrow_mut().class_methods.insert(TypeId::of::<T>(), HashMap::new());
        }

        self.def_method::<T, _>("dup", |_mruby, slf| {
            slf.clone()
        });
    }

    fn def_method<T: Any, F>(&self, name: &str,
                             method: F) where F: Fn(MrubyType, Value) -> Value + 'static {
        {
            let sym = unsafe {
                mrb_intern(self.borrow().mrb, name.as_ptr(), name.len())
            };

            let mut borrow = self.borrow_mut();

            let methods = match borrow.methods.get_mut(&TypeId::of::<T>()) {
                Some(methods) => methods,
                None          => panic!("Class not found.")
            };

            methods.insert(sym, Rc::new(method));
        }

        extern "C" fn call_method<T: Any>(mrb: *const MrState, slf: MrValue) -> MrValue {
            unsafe {
                let ptr = mrb_ext_get_ud(mrb);
                let mruby = mem::transmute::<*const u8, MrubyType>(ptr);

                let result = {
                    let value = Value::new(mruby.clone(), slf);

                    let method = {
                        let borrow = mruby.borrow();

                        let methods = match borrow.methods.get(&TypeId::of::<T>()) {
                            Some(methods) => methods,
                            None          => {
                                return mruby.raise("TypeError", "Class not found.").value
                            }
                        };

                        let sym = mrb_ext_get_mid(mrb);

                        match methods.get(&sym) {
                            Some(method) => method.clone(),
                            None         => {
                                return mruby.raise("TypeError", "Method not found.").value
                            }
                        }
                    };

                    match panic::recover(AssertRecoverSafe::new(|| method(mruby.clone(), value).value)) {
                        Ok(value)  => value,
                        Err(error) => {
                            let message = match error.downcast_ref::<&'static str>() {
                                Some(s) => *s,
                                None    => match error.downcast_ref::<String>() {
                                    Some(s) => &s[..],
                                    None    => ""
                                }
                            };

                            mruby.raise("RustPanic", message).value
                        }
                    }
                };

                mem::forget(mruby);

                result
            }
        }

        let borrow = self.borrow();

        let class = match borrow.classes.get(&TypeId::of::<T>()) {
            Some(class) => class,
            None       => panic!("Class not found.")
        };

        unsafe {
            mrb_define_method(borrow.mrb, class.0, CString::new(name).unwrap().as_ptr(),
                              call_method::<T>, 1 << 12);
        }
    }

    fn def_class_method<T: Any, F>(&self, name: &str, method: F)
        where F: Fn(MrubyType, Value) -> Value + 'static {
        {
            let sym = unsafe {
                mrb_intern(self.borrow().mrb, name.as_ptr(), name.len())
            };

            let mut borrow = self.borrow_mut();

            let methods = match borrow.class_methods.get_mut(&TypeId::of::<T>()) {
                Some(methods) => methods,
                None          => panic!("Class not found.")
            };

            methods.insert(sym, Rc::new(method));
        }

        extern "C" fn call_class_method<T: Any>(mrb: *const MrState, slf: MrValue) -> MrValue {
            unsafe {
                let ptr = mrb_ext_get_ud(mrb);
                let mruby = mem::transmute::<*const u8, MrubyType>(ptr);

                let result = {
                    let value = Value::new(mruby.clone(), slf);

                    let method = {
                        let borrow = mruby.borrow();

                        let methods = match borrow.class_methods.get(&TypeId::of::<T>()) {
                            Some(methods) => methods,
                            None          => {
                                return mruby.raise("TypeError", "Class not found.").value
                            }
                        };

                        let sym = mrb_ext_get_mid(mrb);

                        match methods.get(&sym) {
                            Some(method) => method.clone(),
                            None         => {
                                return mruby.raise("TypeError", "Method not found.").value
                            }
                        }
                    };

                    match panic::recover(AssertRecoverSafe::new(|| method(mruby.clone(), value).value)) {
                        Ok(value)  => value,
                        Err(error) => {
                            let message = match error.downcast_ref::<&'static str>() {
                                Some(s) => *s,
                                None    => match error.downcast_ref::<String>() {
                                    Some(s) => &s[..],
                                    None    => ""
                                }
                            };

                            mruby.raise("RustPanic", message).value
                        }
                    }
                };

                mem::forget(mruby);

                result
            }
        }

        let borrow = self.borrow();

        let class = match borrow.classes.get(&TypeId::of::<T>()) {
            Some(class) => class,
            None       => panic!("Class not found.")
        };

        unsafe {
            mrb_define_class_method(borrow.mrb, class.0, CString::new(name).unwrap().as_ptr(),
                                    call_class_method::<T>, 1 << 12);
        }
    }

    #[inline]
    fn class_name<T: Any>(&self) -> Result<String, MrubyError> {
        let borrow = self.borrow();

        match borrow.classes.get(&TypeId::of::<T>()) {
            Some(class) => Ok(class.2.clone()),
            None        => Err(MrubyError::Undef)
        }
    }

    #[inline]
    fn nil(&self) -> Value {
        unsafe {
            Value::new(self.clone(), MrValue::nil())
        }
    }

    #[inline]
    fn bool(&self, value: bool) -> Value {
        unsafe {
            Value::new(self.clone(), MrValue::bool(value))
        }
    }

    #[inline]
    fn fixnum(&self, value: i32) -> Value {
        unsafe {
            Value::new(self.clone(), MrValue::fixnum(value))
        }
    }

    #[inline]
    fn float(&self, value: f64) -> Value {
        unsafe {
            Value::new(self.clone(), MrValue::float(self.borrow().mrb, value))
        }
    }

    #[inline]
    fn string(&self, value: &str) -> Value {
        unsafe {
            Value::new(self.clone(), MrValue::string(self.borrow().mrb, value))
        }
    }

    #[inline]
    fn symbol(&self, value: &str) -> Value {
        unsafe {
            Value::new(self.clone(), MrValue::symbol(self.borrow().mrb, value))
        }
    }

    #[inline]
    fn obj<T: Any>(&self, obj: T) -> Value {
        let borrow = self.borrow();

        let class = match borrow.classes.get(&TypeId::of::<T>()) {
            Some(class) => class,
            None       => panic!("Class not found.")
        };

        unsafe {
            Value::new(self.clone(), MrValue::obj(borrow.mrb, class.0 as *const MrClass, obj,
                                                  &class.1))
        }
    }

    #[inline]
    fn option<T: Any>(&self, obj: Option<T>) -> Value {
        match obj {
            Some(obj) => self.obj(obj),
            None      => self.nil()
        }
    }

    #[inline]
    fn array(&self, value: Vec<Value>) -> Value {
        let array: Vec<MrValue> = value.iter().map(|value| {
            value.value
        }).collect();

        unsafe {
            Value::new(self.clone(), MrValue::array(self.borrow().mrb, array))
        }
    }
}

impl Drop for Mruby {
    fn drop(&mut self) {
        self.close();
    }
}

/// A `struct` that wraps around any mruby variable.
///
/// `Values` are created from the `Mruby` instance:
///
/// * [`nil`](../mrusty/trait.MrubyImpl.html#tymethod.nil)
/// * [`bool`](../mrusty/trait.MrubyImpl.html#tymethod.bool)
/// * [`fixnum`](../mrusty/trait.MrubyImpl.html#tymethod.fixnum)
/// * [`float`](../mrusty/trait.MrubyImpl.html#tymethod.float)
/// * [`string`](../mrusty/trait.MrubyImpl.html#tymethod.string)
/// * [`obj`](../mrusty/trait.MrubyImpl.html#tymethod.obj)
/// * [`option`](../mrusty/trait.MrubyImpl.html#tymethod.option)
/// * [`array`](../mrusty/trait.MrubyImpl.html#tymethod.array)
///
/// # Examples
///
/// ```
/// # use mrusty::Mruby;
/// # use mrusty::MrubyImpl;
/// let mruby = Mruby::new();
/// let result = mruby.run("true").unwrap(); // Value
///
/// // Values need to be unwrapped in order to make sure they have the right mruby type.
/// assert_eq!(result.to_bool().unwrap(), true);
/// ```
pub struct Value {
    mruby: MrubyType,
    value: MrValue
}

impl Value {
    /// Not meant to be called directly.
    #[doc(hidden)]
    pub fn new(mruby: MrubyType, value: MrValue) -> Value {
        Value {
            mruby: mruby,
            value: value
        }
    }

    /// Initializes the `self` mruby object passed to `initialize` with a Rust object of type `T`.
    ///
    /// *Note:* `T` must be defined on the current `Mruby` with `def_class`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate mrusty;
    /// use mrusty::*;
    ///
    /// # fn main() {
    /// let mruby = Mruby::new();
    ///
    /// struct Cont {
    ///     value: i32
    /// };
    ///
    /// mruby.def_class::<Cont>("Container");
    /// mruby.def_method::<Cont, _>("initialize", mrfn!(|_mruby, slf: Value, v: i32| {
    ///     let cont = Cont { value: v };
    ///
    ///     slf.init(cont) // Return the same slf value.
    /// }));
    ///
    /// let result = mruby.run("Container.new 3").unwrap();
    ///
    /// assert_eq!(result.to_obj::<Cont>().unwrap().value, 3);
    /// # }
    /// ```
    pub fn init<T: Any>(self, obj: T) -> Value {
        unsafe {
            let rc = Rc::new(obj);
            let ptr = mem::transmute::<Rc<T>, *const u8>(rc);

            let borrow = self.mruby.borrow();

            let class = match borrow.classes.get(&TypeId::of::<T>()) {
                Some(class) => class,
                None       => panic!("Class not found.")
            };

            let data_type = &class.1;

            mrb_ext_data_init(&self.value as *const MrValue, ptr, data_type as *const MrDataType);
        }

        self
    }

    /// Calls method `name` on a `Value` passing `args`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// let one = mruby.fixnum(1);
    /// let result = one.call("+", vec![mruby.fixnum(2)]).unwrap();
    ///
    /// assert_eq!(result.to_i32().unwrap(), 3);
    /// ```
    pub fn call(&self, name: &str, args: Vec<Value>) -> Result<Value, MrubyError> {
        unsafe {
            let sym = mrb_intern(self.mruby.borrow().mrb, name.as_ptr(), name.len());

            let args: Vec<MrValue> = args.iter().map(|value| value.value).collect();

            let result = mrb_funcall_argv(self.mruby.borrow().mrb, self.value, sym,
                                          args.len() as i32, args.as_ptr());

            let exc = mrb_ext_get_exc(self.mruby.borrow().mrb);

            match exc.typ {
                MrType::MRB_TT_FALSE => {
                    Ok(Value::new(self.mruby.clone(), result))
                },
                _  => Err(MrubyError::Runtime(exc.to_str(self.mruby.borrow().mrb).unwrap()
                                                 .to_owned()))
            }
        }
    }

    /// Calls method `name` on a `Value` passing `args`. If call fails, mruby will be left to
    /// handle the exception.
    ///
    /// # Examples
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// let one = mruby.string("");
    /// one.call("+", vec![mruby.fixnum(1)]);
    /// ```
    pub fn call_unchecked(&self, name: &str, args: Vec<Value>) -> Value {
        unsafe {
            let sym = mrb_intern(self.mruby.borrow().mrb, name.as_ptr(), name.len());

            let args: Vec<MrValue> = args.iter().map(|value| value.value).collect();

            let result = mrb_funcall_argv(self.mruby.borrow().mrb, self.value, sym,
                                          args.len() as i32, args.as_ptr());

            Value::new(self.mruby.clone(), result)
        }
    }

    /// Returns the name of the mruby `Class` as a `&str`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// let one = mruby.run("1").unwrap();
    /// assert_eq!(one.type_name(), "Fixnum");
    /// ```
    pub fn type_name(&self) -> &str {
        let string = self.call_unchecked("class", vec![]).call_unchecked("to_s", vec![]);

        string.to_str().unwrap()
    }

    /// Casts a `Value` and returns a `bool` in an `Ok` or an `Err` if the types mismatch.
    ///
    /// # Example
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    /// let result = mruby.run("
    ///   def pos(n)
    ///     n > 0
    ///   end
    ///
    ///   pos 1
    /// ").unwrap();
    ///
    /// assert_eq!(result.to_bool().unwrap(), true);
    /// ```
    #[inline]
    pub fn to_bool(&self) -> Result<bool, MrubyError> {
        unsafe {
            self.value.to_bool()
        }
    }

    /// Casts a `Value` and returns an `i32` in an `Ok` or an `Err` if the types mismatch.
    ///
    /// # Example
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    /// let result = mruby.run("
    ///   def fact(n)
    ///     n > 1 ? fact(n - 1) * n : 1
    ///   end
    ///
    ///   fact 5
    /// ").unwrap();
    ///
    /// assert_eq!(result.to_i32().unwrap(), 120);
    /// ```
    #[inline]
    pub fn to_i32(&self) -> Result<i32, MrubyError> {
        unsafe {
            self.value.to_i32()
        }
    }

    /// Casts a `Value` and returns an `f64` in an `Ok` or an `Err` if the types mismatch.
    ///
    /// # Example
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    /// let result = mruby.run("
    ///   3 / 2.0
    /// ").unwrap();
    ///
    /// assert_eq!(result.to_f64().unwrap(), 1.5);
    /// ```
    #[inline]
    pub fn to_f64(&self) -> Result<f64, MrubyError> {
        unsafe {
            self.value.to_f64()
        }
    }

    /// Casts a `Value` and returns a `&str` in an `Ok` or an `Err` if the types mismatch.
    ///
    /// # Example
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    /// let result = mruby.run("
    ///   [1, 2, 3].map(&:to_s).join
    /// ").unwrap();
    ///
    /// assert_eq!(result.to_str().unwrap(), "123");
    /// ```
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    /// let result = mruby.run(":symbol").unwrap();
    ///
    /// assert_eq!(result.to_str().unwrap(), "symbol");
    /// ```
    #[inline]
    pub fn to_str<'a>(&self) -> Result<&'a str, MrubyError> {
        unsafe {
            self.value.to_str(self.mruby.borrow().mrb)
        }
    }

    /// Casts mruby `Value` of `Class` `name` to Rust type `Rc<T>`.
    ///
    /// *Note:* `T` must be defined on the current `Mruby` with `def_class`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// struct Cont {
    ///     value: i32
    /// }
    ///
    /// mruby.def_class::<Cont>("Container");
    ///
    /// let value = mruby.obj(Cont { value: 3 });
    /// let cont = value.to_obj::<Cont>().unwrap();
    ///
    /// assert_eq!(cont.value, 3);
    /// ```
    #[inline]
    pub fn to_obj<T: Any>(&self) -> Result<Rc<T>, MrubyError> {
        unsafe {
            let borrow = self.mruby.borrow();

            let class = match borrow.classes.get(&TypeId::of::<T>()) {
                Some(class) => class,
                None        => {
                    return Err(MrubyError::Undef)
                }
            };

            let class_name = self.type_name();

            if class_name != class.2 {
                return Err(MrubyError::Undef)
            }

            self.value.to_obj::<T>(borrow.mrb, &class.1)
        }
    }

    /// Casts mruby `Value` of `Class` `name` to Rust `Option` of `Rc<T>`.
    ///
    /// *Note:* `T` must be defined on the current `Mruby` with `def_class`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    ///
    /// struct Cont {
    ///     value: i32
    /// }
    ///
    /// mruby.def_class::<Cont>("Container");
    ///
    /// let value = mruby.obj(Cont { value: 3 });
    /// let cont = value.to_option::<Cont>().unwrap();
    ///
    /// assert_eq!(cont.unwrap().value, 3);
    /// assert!(mruby.nil().to_option::<Cont>().unwrap().is_none());
    /// ```
    #[inline]
    pub fn to_option<T: Any>(&self) -> Result<Option<Rc<T>>, MrubyError> {
        if self.value.typ == MrType::MRB_TT_DATA {
            self.to_obj::<T>().map(|obj| Some(obj))
        } else {
            Ok(None)
        }
    }

    /// Casts mruby `Value` of `Class` `Array` to Rust type `Vec<Value>`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use mrusty::Mruby;
    /// # use mrusty::MrubyImpl;
    /// let mruby = Mruby::new();
    /// let result = mruby.run("
    ///   [1, 2, 3].map(&:to_s)
    /// ").unwrap();
    ///
    /// assert_eq!(result.to_vec().unwrap(), vec![
    ///     mruby.string("1"),
    ///     mruby.string("2"),
    ///     mruby.string("3")
    /// ]);
    /// ```
    #[inline]
    pub fn to_vec(&self) -> Result<Vec<Value>, MrubyError> {
        unsafe {
            self.value.to_vec(self.mruby.borrow().mrb).map(|vec| {
                vec.iter().map(|mrvalue| {
                    Value::new(self.mruby.clone(), *mrvalue)
                }).collect()
            })
        }
    }
}

use std::fmt;

impl Clone for Value {
    fn clone(&self) -> Value {
        if self.value.typ == MrType::MRB_TT_DATA {
            unsafe {
                let ptr = mrb_ext_data_ptr(self.value);
                let rc = mem::transmute::<*const u8, Rc<c_void>>(ptr);

                rc.clone();

                mem::forget(rc);
            }
        }

        Value::new(self.mruby.clone(), self.value.clone())
    }
}

impl PartialEq<Value> for Value {
    fn eq(&self, other: &Value) -> bool {
        let result = self.call("==", vec![other.clone()]).unwrap();

        result.to_bool().unwrap()
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Value {{ {:?} }}", self.value)
    }
}
