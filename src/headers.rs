use axum::http;
use headers::{Header, HeaderName, HeaderValue};
use mime::Mime;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Accept(pub Vec<Mime>);

impl Header for Accept {
    fn name() -> &'static HeaderName {
        &http::header::ACCEPT
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, headers::Error>
    where
        I: Iterator<Item = &'i HeaderValue>,
    {
        let mut mime_types = Vec::new();
        for value in values {
            let mimes_str = value.to_str().map_err(|_| headers::Error::invalid())?;
            for mime_str in mimes_str.split(',') {
                let mime_str = mime_str.trim();
                if mime_str.is_empty() {
                    continue;
                }
                let mime: Mime = mime_str.parse().map_err(|_| headers::Error::invalid())?;
                mime_types.push(mime);
            }
        }

        if mime_types.is_empty() {
            return Err(headers::Error::invalid());
        }

        Ok(Accept(mime_types))
    }

    fn encode<E>(&self, values: &mut E)
    where
        E: Extend<HeaderValue>,
    {
        let str = self
            .0
            .iter()
            .map(|mime| mime.to_string())
            .collect::<Vec<_>>()
            .join(", ");

        let value = HeaderValue::from_str(&str);
        if let Ok(value) = value {
            values.extend(std::iter::once(value));
        }
    }
}
