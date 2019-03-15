// Copyright 2015-2016 Brian Smith.
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

//! untrusted.rs: Safe, fast, zero-panic, zero-crashing, zero-allocation
//! parsing of untrusted inputs in Rust.
//!
//! <code>git clone https://github.com/briansmith/untrusted</code>
//!
//! untrusted.rs goes beyond Rust's normal safety guarantees by  also
//! guaranteeing that parsing will be panic-free, as long as
//! `untrusted::Input::as_slice_less_safe()` is not used. It avoids copying
//! data and heap allocation and strives to prevent common pitfalls such as
//! accidentally parsing input bytes multiple times. In order to meet these
//! goals, untrusted.rs is limited in functionality such that it works best for
//! input languages with a small fixed amount of lookahead such as ASN.1, TLS,
//! TCP/IP, and many other networking, IPC, and related protocols. Languages
//! that require more lookahead and/or backtracking require some significant
//! contortions to parse using this framework. It would not be realistic to use
//! it for parsing programming language code, for example.
//!
//! The overall pattern for using untrusted.rs is:
//!
//! 1. Write a recursive-descent-style parser for the input language, where the
//!    input data is given as a `&mut untrusted::Reader` parameter to each
//!    function. Each function should have a return type of `Result<V, E>` for
//!    some value type `V` and some error type `E`, either or both of which may
//!    be `()`. Functions for parsing the lowest-level language constructs
//!    should be defined. Those lowest-level functions will parse their inputs
//!    using `::read_byte()`, `Reader::peek()`, and similar functions.
//!    Higher-level language constructs are then parsed by calling the
//!    lower-level functions in sequence.
//!
//! 2. Wrap the top-most functions of your recursive-descent parser in
//!    functions that take their input data as an `untrusted::Input`. The
//!    wrapper functions should call the `Input`'s `read_all` (or a variant
//!    thereof) method. The wrapper functions are the only ones that should be
//!    exposed outside the parser's module.
//!
//! 3. After receiving the input data to parse, wrap it in an `untrusted::Input`
//!    using `untrusted::Input::from()` as early as possible. Pass the
//!    `untrusted::Input` to the wrapper functions when they need to be parsed.
//!
//! In general parsers built using `untrusted::Reader` do not need to explicitly
//! check for end-of-input unless they are parsing optional constructs, because
//! `Reader::read_byte()` will return `Err(EndOfInput)` on end-of-input.
//! Similarly, parsers using `untrusted::Reader` generally don't need to check
//! for extra junk at the end of the input as long as the parser's API uses the
//! pattern described above, as `read_all` and its variants automatically check
//! for trailing junk. `Reader::skip_to_end()` must be used when any remaining
//! unread input should be ignored without triggering an error.
//!
//! untrusted.rs works best when all processing of the input data is done
//! through the `untrusted::Input` and `untrusted::Reader` types. In
//! particular, avoid trying to parse input data using functions that take
//! byte slices. However, when you need to access a part of the input data as
//! a slice to use a function that isn't written using untrusted.rs,
//! `Input::as_slice_less_safe()` can be used.
//!
//! It is recommend to use `use untrusted;` and then `untrusted::Input`,
//! `untrusted::Reader`, etc., instead of using `use untrusted::*`. Qualifying
//! the names with `untrusted` helps remind the reader of the code that it is
//! dealing with *untrusted* input.
//!
//! # Examples
//!
//! [*ring*](https://github.com/briansmith/ring)'s parser for the subset of
//! ASN.1 DER it needs to understand,
//! [`ring::der`](https://github.com/briansmith/ring/blob/master/src/der.rs),
//! is built on top of untrusted.rs. *ring* also uses untrusted.rs to parse ECC
//! public keys, RSA PKCS#1 1.5 padding, and for all other parsing it does.
//!
//! All of [webpki](https://github.com/briansmith/webpki)'s parsing of X.509
//! certificates (also ASN.1 DER) is done using untrusted.rs.

#![doc(html_root_url = "https://briansmith.org/rustdoc/")]
// `#[derive(...)]` uses `#[allow(unused_qualifications)]` internally.
#![deny(unused_qualifications)]
#![forbid(
    anonymous_parameters,
    box_pointers,
    legacy_directory_ownership,
    missing_docs,
    trivial_casts,
    trivial_numeric_casts,
    unsafe_code,
    unstable_features,
    unused_extern_crates,
    unused_import_braces,
    unused_results,
    variant_size_differences,
    warnings
)]
#![no_std]

/// A wrapper around `&'a [u8]` that helps in writing panic-free code.
///
/// No methods of `Input` will ever panic.
#[derive(Clone, Copy, Debug, Eq)]
pub struct Input<'a>(&'a [u8]);

impl Input<'static> {
    fn empty() -> Self { Self(&[]) }
}

impl<'a> Input<'a> {
    /// Construct a new `Input` for the given input `bytes`.
    pub fn from(bytes: &'a [u8]) -> Self {
        // This limit is important for avoiding integer overflow. In particular,
        // `Reader` assumes that an `i + 1 > i` if `input.value.get(i)` does
        // not return `None`. According to the Rust language reference, the
        // maximum object size is `core::isize::MAX`, and in practice it is
        // impossible to create an object of size `core::usize::MAX` or larger.
        debug_assert!(bytes.len() < core::usize::MAX);
        Self(bytes)
    }

    /// Returns the first byte of the input, or `None` if it is empty.
    #[inline]
    pub fn first(&self) -> Option<&u8> { self.0.first() }

    /// Returns `true` if the input is empty and false otherwise.
    #[inline]
    pub fn is_empty(&self) -> bool { self.0.is_empty() }

    /// Returns the length of the `Input`.
    #[inline]
    pub fn len(&self) -> usize { self.0.len() }

    /// Calls `read` with the given input as a `Reader`, ensuring that `read`
    /// consumed the entire input. If `read` does not consume the entire input,
    /// `incomplete_read` is returned.
    pub fn read_all<F, R, E>(&self, incomplete_read: E, read: F) -> Result<R, E>
    where
        F: FnOnce(&mut Reader<'a>) -> Result<R, E>,
    {
        let mut input = Reader::new(*self);
        let result = read(&mut input)?;
        if input.at_end() {
            Ok(result)
        } else {
            Err(incomplete_read)
        }
    }

    /// Returns the first byte and the rest of the bytes of the input, or
    /// `None` if it is empty.
    #[inline]
    pub fn split_first(&self) -> Option<(u8, Self)> {
        self.0.split_first().map(|(h, t)| (*h, Self(t)))
    }

    /// Splits the input into two parts at position `i`, or returns `None` if
    /// `i` is out of bounds.
    #[inline]
    pub fn split_at(&self, i: usize) -> Option<(Self, Self)> {
        if self.0.len() < i {
            return None;
        }
        let (before, after) = self.0.split_at(i);
        Some((Self(before), Self(after)))
    }

    /// Access the input as a slice so it can be processed by functions that
    /// are not written using the Input/Reader framework.
    #[inline]
    pub fn as_slice_less_safe(&self) -> &'a [u8] { self.0 }
}

// #[derive(PartialEq)] would result in lifetime bounds that are
// unnecessarily restrictive; see
// https://github.com/rust-lang/rust/issues/27950.
impl PartialEq<Input<'_>> for Input<'_> {
    #[inline]
    fn eq(&self, other: &Input) -> bool { self.as_slice_less_safe() == other.as_slice_less_safe() }
}

// https://github.com/rust-lang/rust/issues/27950
impl PartialEq<&[u8]> for Input<'_> {
    #[inline]
    fn eq(&self, other: &&[u8]) -> bool { self.as_slice_less_safe() == *other }
}

/// Calls `read` with the given input as a `Reader`, ensuring that `read`
/// consumed the entire input. When `input` is `None`, `read` will be
/// called with `None`.
pub fn read_all_optional<'a, F, R, E>(
    input: Option<Input<'a>>, incomplete_read: E, read: F,
) -> Result<R, E>
where
    F: FnOnce(Option<&mut Reader<'a>>) -> Result<R, E>,
{
    match input {
        Some(input) => {
            let mut input = Reader::new(input);
            let result = read(Some(&mut input))?;
            if input.at_end() {
                Ok(result)
            } else {
                Err(incomplete_read)
            }
        },
        None => read(None),
    }
}

/// A read-only, forward-only cursor into the data in an `Input`.
///
/// Using `Reader` to parse input helps to ensure that no byte of the input
/// will be accidentally processed more than once. Using `Reader` in
/// conjunction with `read_all` and `read_all_optional` helps ensure that no
/// byte of the input is accidentally left unprocessed. The methods of `Reader`
/// never panic, so `Reader` also assists the writing of panic-free code.
#[derive(Debug)]
pub struct Reader<'a>(Input<'a>);

impl<'a> Reader<'a> {
    /// Construct a new Reader for the given input. Use `read_all` or
    /// `read_all_optional` instead of `Reader::new` whenever possible.
    #[inline]
    pub fn new(input: Input<'a>) -> Self { Self(input) }

    /// Returns `true` if the reader is at the end of the input, and `false`
    /// otherwise.
    #[inline]
    pub fn at_end(&self) -> bool { self.0.is_empty() }

    /// Returns `true` if there is at least one more byte in the input and that
    /// byte is equal to `b`, and false otherwise.
    #[inline]
    pub fn peek(&self, b: u8) -> bool { self.0.first().map(|b| *b) == Some(b) }

    /// Reads the next input byte.
    ///
    /// Returns `Ok(b)` where `b` is the next input byte, or `Err(EndOfInput)`
    /// if the `Reader` is at the end of the input.
    #[inline]
    pub fn read_byte(&mut self) -> Result<u8, EndOfInput> {
        let (h, t) = self.0.split_first().ok_or(EndOfInput)?;
        self.0 = t;
        Ok(h)
    }

    /// Skips `num_bytes` of the input, returning the skipped input as an
    /// `Input`.
    ///
    /// Returns `Ok(i)` if there are at least `num_bytes` of input remaining,
    /// and `Err(EndOfInput)` otherwise.
    #[inline]
    pub fn read_bytes(&mut self, num_bytes: usize) -> Result<Input<'a>, EndOfInput> {
        let (before, after) = self.0.split_at(num_bytes).ok_or(EndOfInput)?;
        self.0 = after;
        Ok(before)
    }

    /// Skips the reader to the end of the input, returning the skipped input
    /// as an `Input`.
    #[inline]
    pub fn read_bytes_to_end(&mut self) -> Input<'a> {
        core::mem::replace(&mut self.0, Input::empty())
    }

    /// Calls `read()` with the given input as a `Reader`. On success, returns a
    /// pair `(bytes_read, r)` where `bytes_read` is what `read()` consumed and
    /// `r` is `read()`'s return value.
    pub fn read_partial<F, R, E>(&mut self, read: F) -> Result<(Input<'a>, R), E>
    where
        F: FnOnce(&mut Reader<'a>) -> Result<R, E>,
    {
        let original = self.0;
        let r = read(self)?;
        let amount_read = original.len().checked_sub(self.0.len()).unwrap();
        let (bytes_read, _) = original.split_at(amount_read).unwrap();
        Ok((bytes_read, r))
    }

    /// Skips `num_bytes` of the input.
    ///
    /// Returns `Ok(i)` if there are at least `num_bytes` of input remaining,
    /// and `Err(EndOfInput)` otherwise.
    #[inline]
    pub fn skip(&mut self, num_bytes: usize) -> Result<(), EndOfInput> {
        self.read_bytes(num_bytes).map(|_| ())
    }

    /// Skips the reader to the end of the input.
    #[inline]
    pub fn skip_to_end(&mut self) -> () { let _ = self.read_bytes_to_end(); }
}

/// The error type used to indicate the end of the input was reached before the
/// operation could be completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EndOfInput;
