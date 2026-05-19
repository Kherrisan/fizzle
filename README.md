# 🔌⚡ Fizzle ⚡🔌 - A Fuzzing Harness for Eliminating Instability in Network Applications


Fizzle is a dynamically-linked shared library for emulating system library functions in a deterministic manner to a *NIX application. It is designed to be preloaded (either via LD_PRELOAD or DYLD_INSERT_LIBRARIES) to interpose libc functions, and it comes with a configurable I/O plugin interface for providing custom inputs to network sockets, nameserver lookups, files and the like.

If you use this work as part of a publication, cite the following [paper](https://www.computer.org/csdl/proceedings-article/sp/2026/606500b689/2bojwkNTstO):

```
@inproceedings{bennett2026fizzle,
    title={{Fizzle: {A} Framework for Deterministic and Reproducible Network Fuzzing}},
    booktitle={{2026 IEEE Symposium on Security and Privacy (S\&P)}},
    author={Bennett, Nathaniel and Tucker, Tyler and Stillman, Carson and Enck, William and Traynor, Patrick and Butler, Kevin R. B.},
    month={may},
    year={2026}
}
```

## Project Status

Fizzle is at this time an experimental research project, not a production-ready tool. Ongoing development is going into making Fizzle handle system library APIs in a more comprehensive and robust manner, and we will happily consider pull requests from those who wish to contribute to the project. If you are looking for production-grade deterministic simulation testing, [Antithesis](https://antithesis.com/) is another deterministic simulation testing tool that offers paid support.

## Security Model

Fizzle is a tool for application fuzzing, **not** sandboxing. Though fizzle interposes system library functions, it can be readily circumvented by an application. In general, you should run Fizzle only if you trust the application you are testing, and are not passing in inputs that may be controlled by an adversary (as there may be internal bugs in Fizzle's handling of userspace simulation that could be affected by such).

## Prerequisites

- [AFLPlusPlus](github.com/AFLplusplus/AFLplusplus) or [libAFL](https://github.com/AFLplusplus/LibAFL) fuzzing engine
- Coverage feedback compiled into the target binary using AFLPlusPlus (or alternatively remove `afl` build feature to use in black-box contexts such as AFL-QEMU)
- Rust nightly (see [https://rust-lang.org/tools/install/](https://rust-lang.org/tools/install/))

## Examples

Several examples of tuning Fizzle for miscellaneous network servers can be found in the companion artifact repository, [determsim/fizzle-artifact](https://https://github.com/determsim/fizzle-artifact). More to follow in this repository.
