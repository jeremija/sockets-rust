use serde::{Deserialize, Serialize};
use serde::{Deserializer, Serializer};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::engine::Engine;

pub fn serialize<S: Serializer>(v: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
    let base64 = BASE64.encode(v);
    String::serialize(&base64, s)
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let base64 = String::deserialize(d)?;
    BASE64
        .decode(base64.as_bytes())
        .map_err(|e| serde::de::Error::custom(e))
}
