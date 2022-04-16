# Dossier

Dossier is an artifact server written in [Rust][rust] using
[BonsaiDb][bonsaidb] as its database. An artifact server is a webserver that is
designed primarily for hosting build artifacts.

## Why does this project exist?

One of the most common ways to deploy documentation, benchmark results, or
generated static websites using GitHub Actions is to build the site in a
`gh-pages` branch. This workflow is easy, and is how BonsaiDb's
[documentation][bonsaidb-docs], [benchmark][bonsaidb-suite]
[results][bonsaidb-commerce], and [user's guide][bonsaidb-guide] are all
deployed this way currently.

The issue arises when cloning the BonsaiDb repository. Using git we can get a
rough idea of the impact on disk usage:

```sh
# Total size of commits reachable from `main`
git rev-list --disk-usage --objects  refs/heads/main
>     4,302,211 (~4.3mb)
# Total size of commits not reachable from `main`.
git rev-list --disk-usage --objects --all --not refs/heads/main
> 3,215,042,793 (~3.2gb)
```

There are alternative ways to solve this problem than building something new.
However, this seemed like a good test project for BonsaiDb that would see
day-to-day use, and there are other tangential features a development server
like this could provide.

## Project Status

This project is in early development. No one should consider using this until at
least the BonsaiDb pages have been re-hosted onto this project.

[rust]: https://rust-lang.org
[bonsaidb]: https://bonsaidb.io/
[bonsaidb-docs]: https://dev.bonsaidb.io/main/docs/bonsaidb/
[bonsaidb-suite]: https://dev.bonsaidb.io/main/benchmarks/suite/report/
[bonsaidb-commerce]: https://dev.bonsaidb.io/main/benchmarks/commerce/
[bonsaidb-guide]: https://dev.bonsaidb.io/main/guide/
