//! Resource names, their URLs and the pages of List methods ([AIP-122], [AIP-158]).
//!
//! A resource name such as `datasets/{d}/versions/{v}` is the path of the resource below
//! `/api/v1/`. Its last segment is the ID.
//!
//! [AIP-122]: https://google.aip.dev/122
//! [AIP-158]: https://google.aip.dev/158

use reqwest::{Method, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::retry::RetryMode;
use crate::{Client, Error};

/// The path below the API base URL that every resource name starts from.
const API_ROOT: &str = "api/v1/";

/// Items per page of a List call: the largest page the API returns.
const PAGE_SIZE: &str = "1000";

/// Returns the ID of a resource name: its last segment, such as `3f2c9a` of
/// `recordings/3f2c9a`.
pub(crate) fn last_segment(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// Returns the URL of `segments` below `/api/v1/`, each segment percent-encoded as a path
/// segment, so an ID may hold any character.
pub(crate) fn url<'a>(
    client: &Client,
    segments: impl IntoIterator<Item = &'a str>,
) -> Result<Url, Error> {
    let mut url = client.url(API_ROOT)?;
    url.path_segments_mut()
        .map_err(|()| Error::Config {
            message: "base URL cannot have a path".to_owned(),
        })?
        .pop_if_empty()
        .extend(segments);
    Ok(url)
}

/// Sends a `GET` to `url` and decodes the JSON response as `T`, retrying network errors,
/// server errors and rate limits.
pub(crate) async fn get<T: DeserializeOwned>(client: &Client, url: Url) -> Result<T, Error> {
    client
        .execute_at(RetryMode::Idempotent, Method::GET, url, None)
        .await
}

/// One page of a List method: the items under the collection key and the token of the
/// next page, empty on the last page.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    #[serde(default)]
    next_page_token: String,
    #[serde(flatten)]
    collections: Map<String, Value>,
}

/// Follows `nextPageToken` from `url` until it is empty and returns the items of the
/// collection `key` of every page, such as `datasets` of `GET /api/v1/datasets`.
pub(crate) async fn list_all<T: DeserializeOwned>(
    client: &Client,
    url: &Url,
    key: &str,
) -> Result<Vec<T>, Error> {
    let mut items = Vec::new();
    let mut token = String::new();
    loop {
        let mut page_url = url.clone();
        {
            let mut query = page_url.query_pairs_mut();
            query.append_pair("pageSize", PAGE_SIZE);
            if !token.is_empty() {
                query.append_pair("pageToken", &token);
            }
        }
        let mut page: Page = get(client, page_url).await?;
        let collection = page.collections.remove(key).unwrap_or(Value::Null);
        if !collection.is_null() {
            let mut decoded: Vec<T> =
                serde_json::from_value(collection).map_err(|error| Error::UnexpectedResponse {
                    status: 200,
                    message: format!(
                        "the `{key}` of the page do not match the expected type: {error}"
                    ),
                })?;
            items.append(&mut decoded);
        }
        if page.next_page_token.is_empty() {
            return Ok(items);
        }
        if page.next_page_token == token {
            return Err(Error::UnexpectedResponse {
                status: 200,
                message: format!("the List of `{key}` repeats its page token"),
            });
        }
        token = page.next_page_token;
    }
}

#[cfg(test)]
mod tests;
