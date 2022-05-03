use std::{collections::HashSet, convert::Infallible, net::SocketAddr, str::Chars};

use bonsaidb::server::{CustomServer, ServerDatabase};
use bonsaidb_files::FileConfig;
use http::{
    header::{CONTENT_LENGTH, IF_NONE_MATCH, LOCATION},
    HeaderValue,
};
use hyper::{
    header::{ALLOW, CONTENT_TYPE, ETAG},
    server::conn::AddrStream,
    service::{make_service_fn, service_fn},
    Body, Method, Request, Response, StatusCode,
};
use mime_guess::MimeGuess;

use crate::{
    schema::{DossierFiles, Metadata},
    CliBackend,
};

pub(crate) fn launch(server: CustomServer<CliBackend>, dossier: ServerDatabase<CliBackend>) {
    let make_service = make_service_fn(move |conn: &AddrStream| {
        let server = server.clone();
        let dossier = dossier.clone();
        let peer_addr = conn.remote_addr();
        async move {
            Ok::<_, Infallible>(service_fn(move |req| {
                get_page_with_error_handling(req, server.clone(), dossier.clone(), peer_addr)
            }))
        }
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let hyper = hyper::Server::bind(&addr).serve(make_service);
    tokio::task::spawn(async move { hyper.await });
}

async fn get_page(
    request: Request<Body>,
    server: CustomServer<CliBackend>,
    pages: ServerDatabase<CliBackend>,
    peer_addr: SocketAddr,
) -> anyhow::Result<Response<Body>> {
    if request.uri().path() == "/_ws" {
        return Ok(server.upgrade_websocket(peer_addr, request).await);
    }

    let path = decode_escaped_path_components(request.uri().path())?;

    let mut file = DossierFiles::load_async(&path, &pages).await?;

    if file.is_none() {
        file = DossierFiles::list_async(&path, &pages)
            .await?
            .into_iter()
            .find(|file| file.name().starts_with("index."));
        if file.is_some() && !path.ends_with('/') {
            // Redirect to the folder's root.
            return Ok(Response::builder()
                .header(LOCATION, format!("{path}/"))
                .status(StatusCode::TEMPORARY_REDIRECT)
                .body(Body::empty())?);
        }
    }

    let file = match file {
        Some(file) => file,
        None => {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from("Not found"))
                .unwrap())
        }
    };

    match request.method() {
        &Method::GET => {
            let (send_body, response) = construct_page_response(
                &request,
                mime_guess::from_path(file.name()),
                file.metadata(),
            );
            if send_body {
                let data = file.contents().await?;
                Ok(response.body(Body::wrap_stream(data)).unwrap())
            } else {
                Ok(response.body(Body::empty()).unwrap())
            }
        }
        &Method::HEAD => {
            let (_, response) = construct_page_response(
                &request,
                mime_guess::from_path(file.name()),
                file.metadata(),
            );

            // TODO get the file's length without retrieiving all blocks
            let data = file.contents().await?;
            Ok(response
                .header(CONTENT_LENGTH, data.len())
                .body(Body::empty())
                .unwrap())
        }
        &Method::OPTIONS => Ok(Response::builder()
            .status(StatusCode::OK)
            .header(ALLOW, "OPTIONS, GET, HEAD")
            .body(Body::empty())
            .unwrap()),
        method => anyhow::bail!("unsupported method: {method:?}"),
    }
}

fn construct_page_response(
    request: &Request<Body>,
    mime_guess: MimeGuess,
    metadata: Option<&Metadata>,
) -> (bool, http::response::Builder) {
    let (send_body, mut response) = match (request.headers().get(IF_NONE_MATCH), metadata) {
        (Some(etags), Some(metadata))
            if parse_etags(etags)
                .unwrap_or_default()
                .contains(&metadata.blake3) =>
        {
            (false, Response::builder().status(StatusCode::NOT_MODIFIED))
        }
        _ => (true, Response::builder().status(StatusCode::OK)),
    };
    if let Some(mime_type) = mime_guess.first_raw() {
        response = response.header(CONTENT_TYPE, mime_type);
    }
    if let Some(metadata) = metadata {
        response = response.header(
            ETAG,
            base64::encode_config(&metadata.blake3, base64::URL_SAFE_NO_PAD),
        );
    }
    (send_body, response)
}

fn parse_etags(etags: &HeaderValue) -> Option<HashSet<[u8; 32]>> {
    let etags = etags.to_str().ok()?;
    let fields = etags.split(',');
    let mut parsed_tags = HashSet::new();
    for quoted_tag in fields {
        let tag = quoted_tag.split('"').nth(1)?;
        if let Ok(tag) = base64::decode(tag) {
            if let Ok(tag) = tag.try_into() {
                parsed_tags.insert(tag);
            }
        }
    }

    Some(parsed_tags)
}

async fn get_page_with_error_handling(
    request: Request<Body>,
    server: CustomServer<CliBackend>,
    pages: ServerDatabase<CliBackend>,
    peer_addr: SocketAddr,
) -> Result<Response<Body>, Infallible> {
    Ok(get_page(request, server, pages, peer_addr)
        .await
        .unwrap_or_else(|err| {
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from(format!("an error occurred: {err}").into_bytes()))
                .unwrap()
        }))
}

fn decode_escaped_path_components(path: &str) -> anyhow::Result<String> {
    PercentDecoder {
        chars: path.chars(),
    }
    .collect()
}

struct PercentDecoder<'a> {
    chars: Chars<'a>,
}

impl<'a> Iterator for PercentDecoder<'a> {
    type Item = anyhow::Result<char>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.chars.next()? {
            '%' => {
                let mut hex = [0; 2];
                hex[0] = match self.chars.next().map(u8::try_from) {
                    Some(Ok(ch)) => ch,
                    _ => return Some(Err(anyhow::anyhow!("invalid percent escape sequence"))),
                };
                hex[1] = match self.chars.next().map(u8::try_from) {
                    Some(Ok(ch)) => ch,
                    _ => return Some(Err(anyhow::anyhow!("invalid percent escape sequence"))),
                };
                match u8::from_str_radix(std::str::from_utf8(&hex).unwrap(), 16) {
                    Ok(b'/') => Some(Err(anyhow::anyhow!("/ is invalid in a path segment"))),
                    Ok(byte) => Some(Ok(char::from(byte))),
                    Err(err) => Some(Err(anyhow::anyhow!(err))),
                }
            }
            '+' => Some(Ok(' ')),
            other => Some(Ok(other)),
        }
    }
}
