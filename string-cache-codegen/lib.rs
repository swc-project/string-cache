// Copyright 2016 The Servo Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
//! A crate to create static string caches at compiletime.
//!
//! # Examples
//!
//! With static atoms:
//!
//! In `Cargo.toml`:
//!
//! ```toml
//! [package]
//! build = "build.rs"
//!
//! [dependencies]
//! string_cache = "0.7"
//!
//! [build-dependencies]
//! string_cache_codegen = "0.4"
//! ```
//!
//! In `build.rs`:
//!
//! ```no_run
//! extern crate string_cache_codegen;
//!
//! use std::env;
//! use std::path::Path;
//!
//! fn main() {
//!     string_cache_codegen::AtomType::new("foo::FooAtom", "foo_atom!")
//!         .atoms(&["foo", "bar"])
//!         .write_to_file(&Path::new(&env::var("OUT_DIR").unwrap()).join("foo_atom.rs"))
//!         .unwrap()
//! }
//! ```
//!
//! In `lib.rs`:
//!
//! ```ignore
//! extern crate string_cache;
//!
//! mod foo {
//!     include!(concat!(env!("OUT_DIR"), "/foo_atom.rs"));
//! }
//! ```
//!
//! The generated code will define a `FooAtom` type and a `foo_atom!` macro.
//! The macro can be used in expression or patterns, with strings listed in `build.rs`.
//! For example:
//!
//! ```ignore
//! fn compute_something(input: &foo::FooAtom) -> u32 {
//!     match *input {
//!         foo_atom!("foo") => 1,
//!         foo_atom!("bar") => 2,
//!         _ => 3,
//!     }
//! }
//! ```
//!

#![recursion_limit = "128"]

extern crate phf_generator;
extern crate phf_shared;
extern crate string_cache_shared as shared;
#[macro_use] extern crate quote;

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Write, BufWriter};
use std::iter;
use std::path::Path;

/// A builder for a static atom set and relevant macros
pub struct AtomType {
    path: String,
    macro_name: String,
    macro_doc: Option<String>,
    atoms: HashSet<String>,
}

impl AtomType {
    /// Constructs a new static atom set builder
    ///
    /// `path` is a path within a crate of the atom type that will be created.
    /// e.g. `"FooAtom"` at the crate root or `"foo::Atom"` if the generated code
    /// is included in a `foo` module.
    ///
    /// `macro_name` must end with `!`.
    ///
    /// For example, `AtomType::new("foo::FooAtom", "foo_atom!")` will generate:
    ///
    /// ```ignore
    /// pub type FooAtom = ::string_cache::Atom<FooAtomStaticSet>;
    /// pub struct FooAtomStaticSet;
    /// impl ::string_cache::StaticAtomSet for FooAtomStaticSet {
    ///     // ...
    /// }
    /// #[macro_export]
    /// macro_rules foo_atom {
    ///    // Expands to: $crate::foo::FooAtom { … }
    /// }
    pub fn new(path: &str, macro_name: &str) -> Self {
        assert!(macro_name.ends_with("!"));
        AtomType {
            path: path.to_owned(),
            macro_name: macro_name[..macro_name.len() - "!".len()].to_owned(),
            macro_doc: None,
            atoms: HashSet::new(),
        }
    }

    /// Add some documentation to the generated macro.
    ///
    /// Note that `docs` should not contain the `///` at the front of normal docs.
    pub fn with_macro_doc(&mut self, docs: &str) -> &mut Self {
        self.macro_doc = Some(docs.to_owned());
        self
    }

    /// Adds an atom to the builder
    pub fn atom(&mut self, s: &str) -> &mut Self {
        self.atoms.insert(s.to_owned());
        self
    }

    /// Adds multiple atoms to the builder
    pub fn atoms<I>(&mut self, iter: I) -> &mut Self
    where I: IntoIterator, I::Item: AsRef<str> {
        self.atoms.extend(iter.into_iter().map(|s| s.as_ref().to_owned()));
        self
    }

    /// Write generated code to `destination`.
    pub fn write_to<W>(&mut self, mut destination: W) -> io::Result<()> where W: Write {
        destination.write_all(
            self.to_tokens()
            .as_str()
            // Insert some newlines to make the generated code slightly easier to read.
            .replace(" [ \"", "[\n\"")
            .replace("\" , ", "\",\n")
            .replace(" ( \"", "\n( \"")
            .replace("; ", ";\n")
            .as_bytes())
    }

    fn to_tokens(&mut self) -> quote::Tokens {
        // `impl Default for Atom` requires the empty string to be in the static set.
        // This also makes sure the set in non-empty,
        // which would cause divisions by zero in rust-phf.
        self.atoms.insert(String::new());

        let atoms: Vec<&str> = self.atoms.iter().map(|s| &**s).collect();
        let hash_state = phf_generator::generate_hash(&atoms);
        let phf_generator::HashState { key, disps, map } = hash_state;
        let atoms: Vec<&str> = map.iter().map(|&idx| atoms[idx]).collect();
        let empty_string_index = atoms.iter().position(|s| s.is_empty()).unwrap() as u32;
        let data = (0..atoms.len()).map(|i| quote::Hex(shared::pack_static(i as u32)));

        let hashes: Vec<u32> =
            atoms.iter().map(|string| {
                let hash = phf_shared::hash(string, key);
                ((hash >> 32) ^ hash) as u32
            }).collect();

        let type_name = if let Some(position) = self.path.rfind("::") {
            &self.path[position + "::".len() ..]
        } else {
            &self.path
        };
        let macro_doc = match self.macro_doc {
            Some(ref doc) => quote!(#[doc = #doc]),
            None => quote!()
        };
        let static_set_name = quote::Ident::from(format!("{}StaticSet", type_name));
        let type_name = quote::Ident::from(type_name);
        let macro_name = quote::Ident::from(&*self.macro_name);
        let path = iter::repeat(quote::Ident::from(&*self.path));

        quote! {
            pub type #type_name = ::string_cache::Atom<#static_set_name>;
            pub struct #static_set_name;
            impl ::string_cache::StaticAtomSet for #static_set_name {
                fn get() -> &'static ::string_cache::PhfStrSet {
                    static SET: ::string_cache::PhfStrSet = ::string_cache::PhfStrSet {
                        key: #key,
                        disps: &#disps,
                        atoms: &#atoms,
                        hashes: &#hashes
                    };
                    &SET
                }
                fn empty_string_index() -> u32 {
                    #empty_string_index
                }
            }
            #macro_doc
            #[macro_export]
            macro_rules! #macro_name {
                #(
                    (#atoms) => {
                        $crate::#path {
                            unsafe_data: #data,
                            phantom: ::std::marker::PhantomData,
                        }
                    };
                )*
            }
        }
    }

    /// Create a new file at `path` and write generated code there.
    ///
    /// Typical usage:
    /// `.write_to_file(&Path::new(&env::var("OUT_DIR").unwrap()).join("foo_atom.rs"))`
    pub fn write_to_file(&mut self, path: &Path) -> io::Result<()> {
        self.write_to(BufWriter::new(try!(File::create(path))))
    }
}
