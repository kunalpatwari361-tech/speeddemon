FROM halloumee/speed-demon-rust:1.99.1-trixie-llvm-tools@sha256:13e4ee7135d8eac6c56476c7d1aee201f36479f4745fd5d0e6b4529f4e13251a AS build
WORKDIR /src
COPY . .
# Profile-guided build: run an instrumented binary on sample data from the published generator,
# then rebuild using that profile.
RUN python3 generate.py --seed 1 --scale sample --out /data && mkdir /out \
 && RUSTFLAGS="-C target-cpu=native -C profile-generate=/tmp/pgo" cargo build --release --offline \
 && target/release/join && llvm-profdata merge -o /tmp/pgo.profdata /tmp/pgo \
 && RUSTFLAGS="-C target-cpu=native -C profile-use=/tmp/pgo.profdata" cargo build --release --offline

FROM debian:13-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad
COPY --from=build /src/target/release/join /app/join
ENTRYPOINT ["/app/join"]
