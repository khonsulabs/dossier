# Dossier

Dossier is an artifact server written in [Rust][rust] using
[BonsaiDb][bonsaidb] as its database. An artifact server is a webserver that is
designed primarily for hosting build artifacts.

## Why does this project exist?

One of the most common ways to deploy documentation, benchmark results, or
generated static websites using GitHub Actions is to build the site in a
`gh-pages` branch. This workflow is easy, and was how BonsaiDb's
[documentation][bonsaidb-docs], [benchmark][bonsaidb-suite]
[results][bonsaidb-commerce], and [user's guide][bonsaidb-guide] were all
deployed.

The issue arpse when cloning the BonsaiDb repository. Using git we could get a
rough idea of the impact on disk usage:

```sh
# Total size of commits reachable from `main`
git rev-list --disk-usage --objects  refs/heads/main
>     4,302,211 (~4.3mb)
# Total size of commits not reachable from `main`.
git rev-list --disk-usage --objects --all --not refs/heads/main
> 3,215,042,793 (~3.2gb)
```

BonsaiDb's repository size was inflated by over 3 gigabytes to support the
`gh-pages` branch.

There were alternative ways to solve this problem than building something new.
However, this seemed like a good test project for BonsaiDb that would see
day-to-day use, and there are other tangential features a development server
like this could provide.

## Project Status

This project is in early development. While BonsaiDb's `gh-pages` branch has
been replaced by this, it is still considered an internal tool despite being
open-source. The design of this project is still early enough that there could
be breaking changes that require manual migration.

## Command Line Guide

This is currently deplyed at [khonsu.dev][docs] using nginx as a reverse proxy.
Eventually, BonsaiDb will be directly servicing the requests. Currently, it
lacks [multi-domain ACME
support](https://github.com/khonsulabs/bonsaidb/issues/173).

An example [nginx config][nginx-config] is available in the repository.

### Server Setup

- Create an administrator

  ```sh
  dossier admin user create your_user --password
  dossier admin user add-group your_user administrators
  ```

- Install [`dossier.service`][systemd-service] into systemd. Customize to
  suite your needs.

  ```sh
  cp dossier.service /etc/systemd/system/
  systemctl enable dossier
  systemctl start dossier
  ```

### Setting up a new project

- Create the project

  ```sh
  dossier -u your_user --url wss://your_domain/_ws project create project_name
  ```

- Create the API Token

  ```sh
  dossier --user your_user --url wss://your_domain/_ws api-token create project_name token_label
  ```

  This command produces an API Token ID and a API Token Secret.

- Syncronize your files

  ```sh
  BONSAIDB_TOKEN_SECRET="api_token_secret" dossier --token api_token_id project sync project_name path/to/local/files /remote/path/
  ```

  This uploads the files to `/project_name/remote/path/`. This command will only
  send files whose contents have changed, and it will delete files present in
  `/project_name/remote/path/`.

  This project's [very empty documentation][docs] is deployed [using GitHub
  Actions][docs-workflow].

[rust]: https://rust-lang.org
[bonsaidb]: https://bonsaidb.io/
[bonsaidb-docs]: https://dev.bonsaidb.io/main/docs/bonsaidb/
[bonsaidb-suite]: https://dev.bonsaidb.io/main/benchmarks/suite/report/
[bonsaidb-commerce]: https://dev.bonsaidb.io/main/benchmarks/commerce/
[bonsaidb-guide]: https://dev.bonsaidb.io/main/guide/
[docs]: https://khonsu.dev/dossier/main/docs/dossier/
[nginx-config]: https://github.com/khonsulabs/dossier/blob/main/dossier.nginx.conf
[systemd-service]: https://github.com/khonsulabs/dossier/blob/main/dossier.service
[docs-workflow]: https://github.com/khonsulabs/dossier/blob/main/.github/workflows/docs.yml
