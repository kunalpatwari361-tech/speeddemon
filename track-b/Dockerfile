FROM python:3.13-slim@sha256:d98cc29d3266e535dd9da1099688d9cb3ca4753813571e071763d17725460962 AS profile-data
WORKDIR /profile
COPY generator.py .
RUN python3 generator.py --seed 2 --scale full --out /profile/data

FROM halloumee/speed-demon-rust:1.99.1-trixie-llvm-tools@sha256:13e4ee7135d8eac6c56476c7d1aee201f36479f4745fd5d0e6b4529f4e13251a AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY vendor vendor
COPY .cargo .cargo
COPY src src
RUN mkdir -p /tmp/pgo && RUSTFLAGS="-C target-cpu=native -Cprofile-generate=/tmp/pgo" cargo build --release --offline
COPY --from=profile-data /profile/data /data
RUN mkdir -p /out && TRACK=b /src/target/release/index
RUN llvm-profdata merge --output=/tmp/pgo/merged.profdata /tmp/pgo/*.profraw
RUN RUSTFLAGS="-C target-cpu=native -Cprofile-use=/tmp/pgo/merged.profdata" cargo build --release --offline

FROM debian:13-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad
COPY --from=build /src/target/release/index /app/index
ENTRYPOINT ["/app/index"]
