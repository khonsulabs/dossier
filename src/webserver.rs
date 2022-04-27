use std::{collections::HashSet, convert::Infallible, net::SocketAddr};

use bonsaidb::server::ServerDatabase;
use bonsaidb_files::FileConfig;
use http::{
    header::{CONTENT_LENGTH, IF_NONE_MATCH},
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

pub(crate) fn launch(dossier: ServerDatabase<CliBackend>) {
    let make_service = make_service_fn(move |_conn: &AddrStream| {
        let dossier = dossier.clone();
        async {
            Ok::<_, Infallible>(service_fn(move |req| {
                get_page_with_error_handling(req, dossier.clone())
            }))
        }
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let hyper = hyper::Server::bind(&addr).serve(make_service);
    tokio::task::spawn(async move { hyper.await });
}

async fn get_page(
    request: Request<Body>,
    pages: ServerDatabase<CliBackend>,
) -> anyhow::Result<Response<Body>> {
    let path = request.uri().path();
    let file = match DossierFiles::load_async(path, &pages).await? {
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
    pages: ServerDatabase<CliBackend>,
) -> Result<Response<Body>, Infallible> {
    Ok(get_page(request, pages).await.unwrap_or_else(|err| {
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from(format!("an error occurred: {err}").into_bytes()))
            .unwrap()
    }))
}
