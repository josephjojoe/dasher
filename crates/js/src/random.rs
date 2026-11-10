//! Random bytes for `crypto.getRandomValues` and `crypto.randomUUID`.

use rquickjs::{Ctx, Exception, Function, Object, TypedArray};

/// `getRandomValues` refuses requests larger than this many bytes.
pub(crate) const MAX_RANDOM_BYTES: usize = 65536;

/// Adds the random-number host function to the prelude's `native` object.
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
    )
}
