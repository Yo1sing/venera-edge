use aes::cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit};
use anyhow::{anyhow, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::Mac;
use rsa::{pkcs8::DecodePrivateKey, traits::PublicKeyParts, Pkcs1v15Encrypt, RsaPrivateKey};
use serde_json::{json, Value};
use sha2::Digest;

pub fn bytes(value: &Value) -> Result<Vec<u8>> {
    let value = value.get("$bytes").unwrap_or(value);
    value
        .as_array()
        .context("Expected byte array")?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|v| u8::try_from(v).ok())
                .ok_or_else(|| anyhow!("Invalid byte"))
        })
        .collect()
}
pub fn binary(bytes: impl AsRef<[u8]>) -> Value {
    json!({"$bytes": bytes.as_ref()})
}

pub fn convert(data: &Value) -> Result<Value> {
    let kind = data["type"].as_str().context("Missing conversion type")?;
    let value = &data["value"];
    let encode = data["isEncode"] == true;
    match kind {
        "utf8" if encode => Ok(binary(
            value.as_str().context("Expected string")?.as_bytes(),
        )),
        "utf8" => Ok(String::from_utf8(bytes(value)?)?.into()),
        "gbk" if encode => {
            let (encoded, _, errors) =
                encoding_rs::GBK.encode(value.as_str().context("Expected string")?);
            ensure!(!errors, "Invalid GBK text");
            Ok(binary(encoded))
        }
        "gbk" => {
            let input = bytes(value)?;
            let (text, _, _) = encoding_rs::GBK.decode(&input);
            Ok(text.to_string().into())
        }
        "base64" if encode => Ok(STANDARD.encode(bytes(value)?).into()),
        "base64" => Ok(binary(
            STANDARD.decode(value.as_str().context("Expected base64 string")?)?,
        )),
        "md5" => Ok(binary(md_digest::Md5::digest(bytes(value)?))),
        "sha1" => Ok(binary(sha1::Sha1::digest(bytes(value)?))),
        "sha256" => Ok(binary(sha2::Sha256::digest(bytes(value)?))),
        "sha512" => Ok(binary(sha2::Sha512::digest(bytes(value)?))),
        "hmac" => {
            let key = bytes(&data["key"])?;
            let input = bytes(value)?;
            macro_rules! hash {
                ($ty:ty) => {{
                    let mut h = <hmac::Hmac<$ty> as Mac>::new_from_slice(&key)
                        .map_err(|_| anyhow!("Invalid HMAC key"))?;
                    h.update(&input);
                    h.finalize().into_bytes().to_vec()
                }};
            }
            let output = match data["hash"].as_str() {
                Some("md5") => hash!(md_digest::Md5),
                Some("sha1") => hash!(sha1::Sha1),
                Some("sha256") => hash!(sha2::Sha256),
                Some("sha512") => hash!(sha2::Sha512),
                _ => return Err(anyhow!("Unsupported HMAC hash")),
            };
            if data["isString"] == true {
                use std::fmt::Write;
                let mut hex = String::with_capacity(output.len() * 2);
                for byte in output {
                    write!(hex, "{byte:02x}")?;
                }
                Ok(hex.into())
            } else {
                Ok(binary(output))
            }
        }
        "aes-ecb" | "aes-cbc" | "aes-cfb" | "aes-ofb" => {
            let key = bytes(&data["key"])?;
            let input = bytes(value)?;
            let iv = if kind == "aes-cbc" || kind == "aes-cfb" {
                bytes(&data["iv"])?
            } else {
                vec![0; 16]
            };
            ensure!(iv.len() == 16, "AES IV must be 16 bytes");
            let segment = if kind == "aes-cfb" || kind == "aes-ofb" {
                data["blockSize"]
                    .as_u64()
                    .context("Missing AES segment size")? as usize
            } else {
                16
            };
            ensure!(
                (1..=16).contains(&segment) && input.len() % segment == 0,
                "Invalid AES block length"
            );
            macro_rules! crypt {
                ($ty:ty) => {{
                    let cipher =
                        <$ty>::new_from_slice(&key).map_err(|_| anyhow!("Invalid AES key"))?;
                    let mut register = iv.clone();
                    let mut output = Vec::with_capacity(input.len());
                    for chunk in input.chunks(segment) {
                        let result = if kind == "aes-ecb" || kind == "aes-cbc" {
                            let mut block = GenericArray::clone_from_slice(chunk);
                            if encode {
                                if kind == "aes-cbc" {
                                    for (a, b) in block.iter_mut().zip(&register) {
                                        *a ^= b;
                                    }
                                }
                                cipher.encrypt_block(&mut block);
                                if kind == "aes-cbc" {
                                    register.copy_from_slice(&block);
                                }
                            } else {
                                cipher.decrypt_block(&mut block);
                                if kind == "aes-cbc" {
                                    for (a, b) in block.iter_mut().zip(&register) {
                                        *a ^= b;
                                    }
                                    register.copy_from_slice(chunk);
                                }
                            }
                            block.to_vec()
                        } else {
                            let mut encrypted = GenericArray::clone_from_slice(&register);
                            cipher.encrypt_block(&mut encrypted);
                            let result: Vec<_> = chunk
                                .iter()
                                .zip(encrypted.iter())
                                .map(|(a, b)| a ^ b)
                                .collect();
                            register.rotate_left(segment);
                            let feedback = if kind == "aes-ofb" {
                                &encrypted[..segment]
                            } else if encode {
                                &result
                            } else {
                                chunk
                            };
                            register[16 - segment..].copy_from_slice(feedback);
                            result
                        };
                        output.extend(result);
                    }
                    output
                }};
            }
            Ok(binary(match key.len() {
                16 => crypt!(aes::Aes128),
                24 => crypt!(aes::Aes192),
                32 => crypt!(aes::Aes256),
                _ => return Err(anyhow!("Invalid AES key size")),
            }))
        }
        "rsa" if !encode => {
            let key = RsaPrivateKey::from_pkcs8_der(
                &STANDARD.decode(data["key"].as_str().context("Expected private key")?)?,
            )?;
            let input = bytes(value)?;
            ensure!(
                input.len() % key.size() == 0,
                "Invalid RSA ciphertext length"
            );
            let mut output = Vec::new();
            for chunk in input.chunks(key.size()) {
                output.extend(key.decrypt(Pkcs1v15Encrypt, chunk)?);
            }
            Ok(binary(output))
        }
        "rsa" => Ok(Value::Null),
        _ => Ok(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conversion_vectors_and_aes_roundtrip() -> Result<()> {
        assert_eq!(
            convert(&json!({"type":"utf8","value":"漫画","isEncode":true}))?,
            binary("漫画".as_bytes())
        );
        assert_eq!(
            convert(&json!({"type":"base64","value":[0,255],"isEncode":true}))?,
            "AP8="
        );
        let input = vec![0; 32];
        for mode in ["aes-ecb", "aes-cbc", "aes-cfb", "aes-ofb"] {
            let mut args = json!({"type":mode,"key":vec![0;16],"iv":vec![0;16],"value":input,"isEncode":true,"blockSize":1});
            let encrypted = convert(&args)?;
            if mode == "aes-ecb" {
                assert_eq!(
                    &bytes(&encrypted)?[..16],
                    &[
                        0x66, 0xe9, 0x4b, 0xd4, 0xef, 0x8a, 0x2c, 0x3b, 0x88, 0x4c, 0xfa, 0x59,
                        0xca, 0x34, 0x2b, 0x2e
                    ]
                );
            }
            args["value"] = encrypted;
            args["isEncode"] = false.into();
            assert_eq!(bytes(&convert(&args)?)?, input);
        }
        Ok(())
    }
}
