//! Host functions for the `crypto` global: random bytes for
//! `crypto.getRandomValues` and `crypto.randomUUID`, and the hashes behind
//! `crypto.subtle.digest`.

use rquickjs::{ArrayBuffer, Ctx, Exception, Function, Object, TypedArray};

/// `getRandomValues` refuses requests larger than this many bytes.
pub(crate) const MAX_RANDOM_BYTES: usize = 65536;

/// Adds the crypto host functions to the prelude's `native` object.
pub(crate) fn install<'js>(ctx: &Ctx<'js>, native: &Object<'js>) -> rquickjs::Result<()> {
    native.set(
        "randomBytes",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, len: usize| -> rquickjs::Result<TypedArray<'js, u8>> {
                let mut bytes = vec![0; len.min(MAX_RANDOM_BYTES)];
                if let Err(e) = getrandom::getrandom(&mut bytes) {
                    return Err(Exception::throw_internal(
                        &ctx,
                        &format!("no source of randomness: {e}"),
                    ));
                }
                TypedArray::new(ctx, bytes)
            },
        )?,
    )?;
    // `digest(name, bytes)`: the hash of `bytes` with the named algorithm
    // (already normalized to `SHA-1`, `SHA-256`, `SHA-384` or `SHA-512`),
    // or undefined for any other name.
    native.set(
        "digest",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             name: String,
             bytes: TypedArray<'js, u8>|
             -> rquickjs::Result<Option<ArrayBuffer<'js>>> {
                let Some(algorithm) = digest_algorithm(&name) else {
                    return Ok(None);
                };
                let hash = ring::digest::digest(algorithm, bytes.as_bytes().unwrap_or_default());
                ArrayBuffer::new_copy(ctx, hash.as_ref()).map(Some)
            },
        )?,
    )
}

fn digest_algorithm(name: &str) -> Option<&'static ring::digest::Algorithm> {
    match name {
        "SHA-1" => Some(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY),
        "SHA-256" => Some(&ring::digest::SHA256),
        "SHA-384" => Some(&ring::digest::SHA384),
        "SHA-512" => Some(&ring::digest::SHA512),
        _ => None,
    }
}
