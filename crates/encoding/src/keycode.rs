//! Keycode is a lexicographical order-preserving binary encoding for use with
//! keys in key/value stores. It is designed for simplicity, not efficiency
//! (i.e. it does not use varints or other compression methods).
//!
//! Ordering is important because it allows limited scans across specific parts
//! of the keyspace, e.g. scanning an individual table or using an index range
//! predicate like `WHERE id < 100`. It also avoids sorting in some cases where
//! the keys are already in the desired order, e.g. in the Raft log.
//!
//! The encoding is not self-describing: the caller must provide a concrete type
//! to decode into, and the binary key must conform to its structure.
//!
//! Keycode supports a subset of primitive data types, encoded as follows:
//!
//! * [`bool`]: `0x00` for `false`, `0x01` for `true`.
//! * [`u64`]: big-endian binary representation.
//! * [`i64`]: big-endian binary, sign bit flipped.
//! * [`f64`]: big-endian binary, sign bit flipped, all flipped if negative.
//! * [`Vec<u8>`]: `0x00` escaped as `0x00ff`, terminated with `0x0000`.
//! * [`String`]: like [`Vec<u8>`].
//! * Sequences: concatenation of contained elements, with no other structure.
//! * Enum: the variant's index as [`u8`], then the content sequence.
//! * [`crate::sql::types::Value`]: like any other enum.
//!
//! The canonical key representation is an enum. For example:
//!
//! ```
//! #[derive(Debug, Deserialize, Serialize)]
//! enum Key {
//!     Foo,
//!     Bar(String),
//!     Baz(bool, u64, #[serde(with = "serde_bytes")] Vec<u8>),
//! }
//! ```
//!
//! Unfortunately, byte strings such as `Vec<u8>` must be wrapped with
//! [`serde_bytes::ByteBuf`] or use the `#[serde(with="serde_bytes")]`
//! attribute. See <https://github.com/serde-rs/bytes>.

use itertools::Either;
use serde::de::{
    DeserializeSeed, IntoDeserializer as _,
    Visitor,
};
use serde::ser::{Impossible, Serialize};

use common_error::errdata;
use common_error::{Error, Result};

/// Serializes a key to a binary Keycode representation.
///
/// In the common case, the encoded key is borrowed for a storage engine call
/// and then thrown away. We could avoid a bunch of allocations by taking a
/// reusable byte vector to encode into and return a reference to it, but we
/// keep it simple.
pub fn serialize<T: Serialize>(key: &T) -> Vec<u8> {
    let mut serializer = Serializer { output: Vec::new() };
    // Panic on failure, as this is a problem with the data structure.
    key.serialize(&mut serializer).expect("key must be serializable");
    serializer.output
}

/// Serializes keys as binary byte vectors.
struct Serializer {
    output: Vec<u8>,
}

impl serde::ser::Serializer for &mut Serializer {
    type Ok = ();
    type Error = Error;

    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleVariant = Self;
    type SerializeTupleStruct = Impossible<(), Error>;
    type SerializeMap = Impossible<(), Error>;
    type SerializeStruct = Impossible<(), Error>;
    type SerializeStructVariant = Impossible<(), Error>;

    /// bool simply uses 1 for true and 0 for false.
    fn serialize_bool(self, v: bool) -> Result<()> {
        self.output.push(if v { 1 } else { 0 });
        Ok(())
    }

    fn serialize_i8(self, _: i8) -> Result<()> {
        unimplemented!()
    }

    fn serialize_i16(self, _: i16) -> Result<()> {
        unimplemented!()
    }

    fn serialize_i32(self, _: i32) -> Result<()> {
        unimplemented!()
    }

    /// i64 uses the big-endian two's complement encoding, but flips the
    /// left-most sign bit such that negative numbers are ordered before
    /// positive numbers.
    ///
    /// The relative ordering of the remaining bits is already correct: -1, the
    /// largest negative integer, is encoded as 01111111...11111111, ordered
    /// after all other negative integers but before positive integers.
    fn serialize_i64(self, v: i64) -> Result<()> {
        let mut bytes = v.to_be_bytes();
        // flip sign bit
        bytes[0] ^= 1 << 7;
        self.output.extend(bytes);
        Ok(())
    }

    fn serialize_u8(self, _: u8) -> Result<()> {
        unimplemented!()
    }

    fn serialize_u16(self, _: u16) -> Result<()> {
        unimplemented!()
    }

    fn serialize_u32(self, _: u32) -> Result<()> {
        unimplemented!()
    }

    /// u64 simply uses the big-endian encoding.
    fn serialize_u64(self, v: u64) -> Result<()> {
        self.output.extend(v.to_be_bytes());
        Ok(())
    }

    fn serialize_f32(self, _: f32) -> Result<()> {
        unimplemented!()
    }

    /// f64 is encoded in big-endian IEEE 754 form, but it flips the sign bit to
    /// order positive numbers after negative numbers, and also flips all other
    /// bits for negative numbers to order them from smallest to largest. NaN is
    /// ordered at the end.
    fn serialize_f64(self, v: f64) -> Result<()> {
        let mut bytes = v.to_be_bytes();
        match v.is_sign_negative() {
            true => bytes.iter_mut().for_each(|b| *b = !*b), // negative, flip all bits
            false => bytes[0] ^= 1 << 7,                     // positive, flip sign bit
        }
        self.output.extend(bytes);
        Ok(())
    }

    fn serialize_char(self, _: char) -> Result<()> {
        unimplemented!()
    }

    /// Strings are encoded like bytes.
    fn serialize_str(self, v: &str) -> Result<()> {
        self.serialize_bytes(v.as_bytes())
    }

    /// Byte slices are terminated by 0x0000, escaping 0x00 as 0x00ff. This
    /// ensures that we can detect the end, and that for two overlapping slices,
    /// the shorter one orders before the longer one.
    ///
    /// We can't use e.g. length prefix encoding, since it doesn't sort correctly.
    fn serialize_bytes(self, v: &[u8]) -> Result<()> {
        let bytes = v
            .iter()
            .flat_map(|&byte| match byte {
                0x00 => Either::Left([0x00, 0xff].into_iter()),
                byte => Either::Right([byte].into_iter()),
            })
            .chain([0x00, 0x00]);
        self.output.extend(bytes);
        Ok(())
    }

    fn serialize_none(self) -> Result<()> {
        unimplemented!()
    }

    fn serialize_some<T>(self, _: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        unimplemented!()
    }

    fn serialize_unit(self) -> Result<()> {
        unimplemented!()
    }

    fn serialize_unit_struct(
        self,
        _name: &'static str,
    ) -> Result<()> {
        unimplemented!()
    }

    /// Enum variants are serialized using their index, as a single byte.
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
    ) -> Result<()> {
        self.output.push(variant_index.try_into()?);
        Ok(())
    }

    fn serialize_newtype_struct<T>(
        self,
        _: &'static str,
        _: &T,
    ) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        unimplemented!()
    }

    /// Newtype variants are serialized using the variant index and inner type.
    fn serialize_newtype_variant<T>(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.serialize_unit_variant(name, variant_index, variant)?;
        value.serialize(self)
    }

    /// Sequences are serialized as the concatenation of the serialized elements.

    fn serialize_seq(
        self,
        _: Option<usize>,
    ) -> Result<Self::SerializeSeq> {
        Ok(self)
    }

    /// Tuples are serialized as the concatenation of the serialized elements.
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple> {
        Ok(self)
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        unimplemented!()
    }

    fn serialize_tuple_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        self.serialize_unit_variant(name, variant_index, variant)?;
        Ok(self)
    }

    fn serialize_map(
        self,
        _len: Option<usize>,
    ) -> Result<Self::SerializeMap> {
        unimplemented!()
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct> {
        unimplemented!()
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant> {
        unimplemented!()
    }
}

/// Sequences simply concatenate the serialized elements, with no external structure.
impl serde::ser::SerializeSeq for &mut Serializer {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<()> {
        Ok(())
    }
}

impl serde::ser::SerializeTuple for &mut Serializer {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<()> {
        Ok(())
    }
}

impl serde::ser::SerializeTupleVariant for &mut Serializer {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<()> {
        Ok(())
    }
}


/// Deserializes keys from byte slices into a given type. The format is not
/// self-describing, so the caller must provide a concrete type to deserialize
/// into.
pub struct Deserializer<'de> {
    input: &'de [u8],
}

impl<'de> Deserializer<'de> {
    /// Creates a deserializer for a byte slice.
    pub fn from_bytes(input: &'de [u8]) -> Self {
        Self { input }
    }

    /// Chops off and returns the next len bytes of the byte slice, or errors if
    /// there aren't enough bytes left.
    fn take_bytes(&mut self, len: usize) -> Result<&[u8]> {
        if self.input.len() < len { 
            return errdata!("insufficient bytes, expected {len} bytes for {:x?}", self.input);
        }
        let bytes = &self.input[..len];
        self.input = &self.input[len..];
        Ok(bytes)
    }
    
    fn decode_next_bytes(&mut self) -> Result<Vec<u8>> {
        let mut decoded = Vec::new();
        let mut iter = self.input.iter().enumerate();
        let taken = loop {
            match iter.next() {
                None => return errdata!("unexpected end of input"),
                Some((_, 0x00)) => match iter.next() {
                    Some((i, 0x00)) => break i + 1,
                    Some((_, 0xff)) => decoded.push(0x00),
                    _ => return errdata!("invalid escape sequence"),
                },
                Some((_, b)) => decoded.push(*b),
            }
        };
        self.input = &self.input[taken..];
        Ok(decoded)
    }
}

/// For details on serialization formats, see Serializer.
impl<'de> serde::de::Deserializer<'de> for &mut Deserializer<'de> {
    type Error = Error;

    fn deserialize_any<V>(self, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        panic!("must provide type, Keycode is not self-describing")
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        visitor.visit_bool(match self.take_bytes(1)?[0] { 
            0x00 => false,
            0x01 => true,
            b => return errdata!("invalid boolean value {b}"),
        })
    }

    fn deserialize_i8<V>(self, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_i16<V>(self, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_i32<V>(self, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        let mut bytes = self.take_bytes(8)?.to_vec();
        bytes[0] ^= 1 << 7; // flip sign bit
        visitor.visit_i64(i64::from_be_bytes(bytes.as_slice().try_into()?))
    }

    fn deserialize_u8<V>(self, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_u16<V>(self, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_u32<V>(self, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        visitor.visit_u64(u64::from_be_bytes(self.take_bytes(8)?.try_into()?))
    }

    fn deserialize_f32<V>(self, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        let mut bytes = self.take_bytes(8)?.to_vec();
        match bytes[0] >> 7 {
            0 => bytes.iter_mut().for_each(|b| *b = !*b), // negative, flip all bits
            1 => bytes[0] ^= 1 << 7,                      // positive, flip sign bit
            _ => panic!("bits can only be 0 or 1"),
        }
        visitor.visit_f64(f64::from_be_bytes(bytes.as_slice().try_into()?))
    }

    fn deserialize_char<V>(self, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        let bytes = self.decode_next_bytes()?;
        visitor.visit_str(&String::from_utf8(bytes)?)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        let bytes = self.decode_next_bytes()?;
        visitor.visit_string(String::from_utf8(bytes)?)
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        let bytes = self.decode_next_bytes()?;
        visitor.visit_bytes(&bytes)
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        let bytes = self.decode_next_bytes()?;
        visitor.visit_byte_buf(bytes)
    }

    fn deserialize_option<V>(self, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_unit<V>(self, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_unit_struct<V>(self, _: &'static str, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_newtype_struct<V>(self, _: &'static str, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        visitor.visit_seq(self)
    }

    fn deserialize_tuple<V>(self, _: usize, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        visitor.visit_seq(self)
    }

    fn deserialize_tuple_struct<V>(self, _: &'static str, _: usize, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_map<V>(self, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_struct<V>(self, _: &'static str, _: &'static [&'static str], _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_enum<V>(self, _: &'static str, _: &'static [&'static str], visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        visitor.visit_enum(self)
    }

    fn deserialize_identifier<V>(self, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }

    fn deserialize_ignored_any<V>(self, _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }
}

/// Sequences are simply deserialized until the byte slice is exhausted.
impl<'de> serde::de::SeqAccess<'de> for Deserializer<'de> {
    type Error = Error;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>>
    where
        T: DeserializeSeed<'de>
    {
        if self.input.is_empty() { 
            return Ok(None);
        }
        seed.deserialize(self).map(Some)
    }
}

/// Enum variants are deserialized by their index.
impl<'de> serde::de::EnumAccess<'de> for &mut Deserializer<'de> {
    type Error = Error;
    type Variant = Self;

    fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self::Variant)>
    where
        V: DeserializeSeed<'de>
    {
        let index = self.take_bytes(1)?[0] as u32;
        let value: Result<_> = seed.deserialize(index.into_deserializer());
        Ok((value?, self))
    }
}

impl<'de> serde::de::VariantAccess<'de> for &mut Deserializer<'de> {
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        Ok(())
    }

    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value>
    where
        T: DeserializeSeed<'de>
    {
        seed.deserialize(&mut *self)
    }

    fn tuple_variant<V>(self, _: usize, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        visitor.visit_seq(self)
    }

    fn struct_variant<V>(self, _: &'static [&'static str], _: V) -> Result<V::Value>
    where
        V: Visitor<'de>
    {
        unimplemented!()
    }
}


#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::f64::consts::PI;

    use paste::paste;
    use serde::{Deserialize, Serialize};
    use serde_bytes::ByteBuf;

    use super::*;
    use sql::types::Value;
}


















