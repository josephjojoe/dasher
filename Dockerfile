# syntax=docker/dockerfile:1
#
# dasher as a static musl binary alone on distroless: about 2 MB of base
# image (CA certificates, tzdata, a nonroot user) plus the binary.
#
#   docker build -t dasher .                 # the image (dasher serve)
#   docker build --target lambda-zip --output dist .
#                                            # dist/bootstrap for a Lambda zip
#
# Builds for the platform being built for: pass --platform linux/amd64 or
# linux/arm64 to choose.

FROM rust:1-alpine AS build
# QuickJS is C, compiled by the cc crate.
RUN apk add --no-cache gcc musl-dev
WORKDIR /src
COPY . .
# The pinned toolchain file would make rustup fetch another toolchain;
# the image's own stable one is what it pins anyway.
RUN rm -f rust-toolchain.toml
# On Alpine the host target is musl, linked statically by default.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p dasher-cli \
    && cp target/release/dasher /dasher

# Just the binary, named as a Lambda custom runtime expects, and the notices
# to ship with it.
FROM scratch AS lambda-zip
COPY --from=build /dasher /bootstrap
COPY LICENSE THIRD-PARTY-LICENSES.txt /

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /dasher /dasher
# The licence, and the notices of what is compiled into the binary.
COPY LICENSE THIRD-PARTY-LICENSES.txt /licenses/
ENV PORT=8080
EXPOSE 8080
USER nonroot
ENTRYPOINT ["/dasher"]
# `lambda` instead runs the image as a Lambda custom runtime.
CMD ["serve"]
