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

BonsaiDb's repository size is inflated by over 3 gigabytes to
support the gh-pages branch.

There are alternative ways to solve this problem than building something new.
However, this seemed like a good test project for BonsaiDb that would see
day-to-day use, and there are other tangential features a development server
like this could provide.

## Project Status

This project is in early development. No one should consider using this until at
least the BonsaiDb pages have been re-hosted onto this project.

## Command Line Guide

This is currently deplyed at [khonsu.dev][docs] using nginx as a reverse proxy.
Eventually, BonsaiDb will be directly servicing the requests. Currently, it
lacks [multi-domain ACME
support](https://github.com/khonsulabs/bonsaidb/issues/173).

An example [nginx config](./dossier.nginx.conf) is available in the repository.

### Server Setup

- Create an administrator

  ```sh
  dossier admin user create your_user --password
  dossier admin user add-group your_user administrators
  ```

- Install [`dossier.service`](./dossier.service) into systemd. Customize to
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
  Actions](./.github/workflows/docs.yml).

[rust]: https://rust-lang.org
[bonsaidb]: https://bonsaidb.io/
[bonsaidb-docs]: https://dev.bonsaidb.io/main/docs/bonsaidb/
[bonsaidb-suite]: https://dev.bonsaidb.io/main/benchmarks/suite/report/
[bonsaidb-commerce]: https://dev.bonsaidb.io/main/benchmarks/commerce/
[bonsaidb-guide]: https://dev.bonsaidb.io/main/guide/
[docs]: https://khonsu.dev/dossier/main/docs/dossier/
