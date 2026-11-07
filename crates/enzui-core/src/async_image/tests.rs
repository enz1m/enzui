use super::*;

type H = u32;

#[test]
fn a_clean_remote_url_is_accepted_and_loads() {
    let policy = UrlPolicy::DEFAULT;
    let mut img: EnzAsyncImage<H> = EnzAsyncImage::new();
    let g = img
        .request_remote("https://cdn.example.com/hero.png", &policy)
        .expect("a clean HTTPS URL must be accepted");
    assert_eq!(*img.state(), ImageState::Loading(g));
    let src = img.source().expect("source stored");
    assert!(src.redacted().contains("cdn.example.com"));
}
