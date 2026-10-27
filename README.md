# websearch

Clearnet search engine: SSRF-safe crawler (the guard as a type), FTS + BM25/PageRank ranking, no-JS UI + JSON API, federation.

Part of the [astrx suite](https://github.com/Vxmdzhb/astrx-suite): one of its engines. The suite repository assembles every engine into the single `astrx` binary (`astrx websearch`) and deploys it.

## Build

    cargo build --release --features net   # the `websearch` binary
    cargo test --all-features

Features: `rand`, `net`. The default build uses the standard library only; `net` (and `rand`) add the suite's two vetted dependencies, tokio and getrandom. CI asserts the default build stays first-party.

## History

This repository was split out of https://github.com/Vxmdzhb/astrx-suite with the full history of its files, each followed back through its renames. The cross-check goldens are explained in [GOLDENS.md](GOLDENS.md).

## License

Released under the [Do Whatever License](LICENSE), with a code of conduct
in the same spirit: [CODE_OF_CONDUCT](CODE_OF_CONDUCT).
