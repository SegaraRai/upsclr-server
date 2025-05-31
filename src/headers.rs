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

#[cfg(test)]
mod tests {
    use super::*;
    use headers::{Header, HeaderValue};

    #[test]
    fn test_accept_header_single_mime() {
        let header_value = HeaderValue::from_static("image/png");
        let mut values = std::iter::once(&header_value);

        let accept = Accept::decode(&mut values).unwrap();
        assert_eq!(accept.0.len(), 1);
        assert_eq!(accept.0[0].type_(), "image");
        assert_eq!(accept.0[0].subtype(), "png");
    }

    #[test]
    fn test_accept_header_multiple_mimes() {
        let header_value = HeaderValue::from_static("image/png, image/jpeg, image/webp");
        let mut values = std::iter::once(&header_value);

        let accept = Accept::decode(&mut values).unwrap();
        assert_eq!(accept.0.len(), 3);

        assert_eq!(accept.0[0].type_(), "image");
        assert_eq!(accept.0[0].subtype(), "png");

        assert_eq!(accept.0[1].type_(), "image");
        assert_eq!(accept.0[1].subtype(), "jpeg");

        assert_eq!(accept.0[2].type_(), "image");
        assert_eq!(accept.0[2].subtype(), "webp");
    }

    #[test]
    fn test_accept_header_with_whitespace() {
        let header_value = HeaderValue::from_static("  image/png  ,  image/jpeg  ");
        let mut values = std::iter::once(&header_value);

        let accept = Accept::decode(&mut values).unwrap();
        assert_eq!(accept.0.len(), 2);
        assert_eq!(accept.0[0].subtype(), "png");
        assert_eq!(accept.0[1].subtype(), "jpeg");
    }

    #[test]
    fn test_accept_header_empty_segments() {
        let header_value = HeaderValue::from_static("image/png,,image/jpeg,");
        let mut values = std::iter::once(&header_value);

        let accept = Accept::decode(&mut values).unwrap();
        assert_eq!(accept.0.len(), 2);
        assert_eq!(accept.0[0].subtype(), "png");
        assert_eq!(accept.0[1].subtype(), "jpeg");
    }

    #[test]
    fn test_accept_header_invalid_mime() {
        let header_value = HeaderValue::from_static("invalid-mime-type");
        let mut values = std::iter::once(&header_value);

        assert!(Accept::decode(&mut values).is_err());
    }

    #[test]
    fn test_accept_header_empty() {
        let header_value = HeaderValue::from_static("");
        let mut values = std::iter::once(&header_value);

        assert!(Accept::decode(&mut values).is_err());
    }

    #[test]
    fn test_accept_header_name() {
        assert_eq!(Accept::name(), &http::header::ACCEPT);
    }

    #[test]
    fn test_accept_header_encode_single() {
        let accept = Accept(vec!["image/png".parse().unwrap()]);
        let mut values = Vec::new();
        accept.encode(&mut values);

        assert_eq!(values.len(), 1);
        assert_eq!(values[0].to_str().unwrap(), "image/png");
    }

    #[test]
    fn test_accept_header_encode_multiple() {
        let accept = Accept(vec![
            "image/png".parse().unwrap(),
            "image/jpeg".parse().unwrap(),
            "image/webp".parse().unwrap(),
        ]);
        let mut values = Vec::new();
        accept.encode(&mut values);

        assert_eq!(values.len(), 1);
        assert_eq!(
            values[0].to_str().unwrap(),
            "image/png, image/jpeg, image/webp"
        );
    }

    #[test]
    fn test_accept_header_round_trip() {
        let original_value = "image/png, image/jpeg, application/json";
        let header_value = HeaderValue::from_static(original_value);
        let mut decode_values = std::iter::once(&header_value);

        let accept = Accept::decode(&mut decode_values).unwrap();

        let mut encode_values = Vec::new();
        accept.encode(&mut encode_values);

        assert_eq!(encode_values.len(), 1);
        let encoded = encode_values[0].to_str().unwrap();

        // The encoded value should contain all the original MIME types
        assert!(encoded.contains("image/png"));
        assert!(encoded.contains("image/jpeg"));
        assert!(encoded.contains("application/json"));
    }
}
