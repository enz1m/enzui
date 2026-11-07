use super::*;
use enzui_core::download::DownloadId;
use enzui_core::download_destination::{
    DestinationBroker, DestinationOutcome, ReservedDownloadDestination,
};
use enzui_core::portal::backend::{BackendReply, MockBackend};
use enzui_core::portal::{PortalParent, PortalResponse};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    scratch: PathBuf,
    executable: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

fn reserved_destination(path: &Path) -> ReservedDownloadDestination {
    let mut backend = MockBackend::new();
    backend.script(BackendReply {
        response: PortalResponse::Success,
        uris: vec![format!("file://{}", path.display())],
    });
    let mut broker = DestinationBroker::new();
    assert_eq!(
        broker.request(DownloadId(1), &mut backend, PortalParent::None, |_| true),
        None
    );
    assert!(matches!(
        broker.pump(&mut backend).as_slice(),
        [(DownloadId(1), DestinationOutcome::Completed(_))]
    ));
    broker
        .destination(DownloadId(1))
        .expect("portal destination")
        .reserve_file()
        .expect("reserve portal destination")
}

fn fixture() -> Fixture {
    let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!(
            "service-extractor-{}-{sequence}",
            std::process::id()
        ));
    std::fs::create_dir_all(&scratch).expect("create extractor fixture directory");
    let executable = scratch.join("yt-dlp-fixture");
    std::fs::write(
        &executable,
        r#"#!/bin/sh
fixture_dir=${0%/*}
mode=download
format=
output=
url=
merge=
batch_file=
positional_url=
saw_ignore=0
saw_plugins=0
saw_remote=0
saw_update=0
saw_cache=0
saw_playlist=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --version) mode=version ;;
    --skip-download) mode=inspect ;;
    --ignore-config) saw_ignore=1 ;;
    --no-plugin-dirs) saw_plugins=1 ;;
    --no-remote-components) saw_remote=1 ;;
    --no-update) saw_update=1 ;;
    --no-cache-dir) saw_cache=1 ;;
    --no-playlist) saw_playlist=1 ;;
    --format) shift; format="$1" ;;
    --output) shift; output="$1" ;;
    --merge-output-format) shift; merge="$1" ;;
    --batch-file) shift; batch_file="$1" ;;
    --) shift; positional_url="$1"; break ;;
  esac
  shift
done
[ "$saw_ignore" = 1 ] && [ "$saw_plugins" = 1 ] && [ "$saw_update" = 1 ] || exit 42
printf '%s' "$PWD" > "$fixture_dir/last-cwd"
/usr/bin/stat -c '%a' . > "$fixture_dir/last-cwd-mode"
if [ "$mode" = version ]; then
  printf '2026.08.19\n'
  exit 0
fi
[ "$saw_remote" = 1 ] && [ "$saw_cache" = 1 ] && [ "$saw_playlist" = 1 ] || exit 42
[ "$batch_file" = - ] || { printf 'source URL did not use stdin\n' >&2; exit 45; }
[ -z "$positional_url" ] || { printf 'source URL reached argv\n' >&2; exit 46; }
IFS= read -r cmdline < /proc/self/cmdline || :
printf '%s' "$cmdline" > "$fixture_dir/last-cmdline"
IFS= read -r url || { printf 'source URL was missing its terminating newline\n' >&2; exit 47; }
extra=
if IFS= read -r extra || [ -n "$extra" ]; then
  printf 'stdin carried more than one source URL\n' >&2
  exit 50
fi
printf '%s' "$url" > "$fixture_dir/last-stdin"
if [ "$mode" = inspect ]; then
  case "$url" in
    'https://alice:private-channel-userinfo@service.test/private-channel?token=private-channel-query')
      printf 'fixture received exact private URL: %s\n' "$url" >&2
      exit 48
      ;;
    'https://service.test/benign-failure')
      printf 'upstream returned status 403 for %s\n' "$url" >&2
      exit 49
      ;;
  esac
  [ "$format" = all ] || { printf 'metadata did not select all formats\n' >&2; exit 44; }
  case "$url" in
    *slow-metadata*) sleep 10 ;;
    *malformed*) printf 'not-the-protocol\n'; exit 0 ;;
    *flood*) head -c 600000 /dev/zero | tr '\0' x; exit 0 ;;
  esac
  cat <<'EOF'
enzui-format-v1	Example_Video	125	22	mp4	1280	720	avc1	mp4a	1234
enzui-format-v1	Example_Video	125	140	m4a	NA	NA	none	mp4a	456
EOF
  exit 0
fi
case "$format" in
  22) [ -z "$merge" ] || exit 43 ;;
  bestvideo+bestaudio/best) [ "$merge" = mkv ] || exit 43 ;;
  *) printf 'wrong format\n' >&2; exit 43 ;;
esac
case "$url" in
  *slow*) sleep 10 ;;
esac
dir=${output%/*}
printf 'enzui-progress-v1\t4\t7\n' >&2
printf 'payload' > "$dir/payload.mp4"
case "$url" in
  *multi*) printf 'foreign' > "$dir/second.mp4" ;;
esac
"#,
    )
    .expect("write extractor fixture");
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&executable, permissions).unwrap();
    Fixture {
        scratch,
        executable,
    }
}

#[test]
fn composite_choice_uses_the_fixed_merge_selector_and_container() {
    let fixture = fixture();
    let backend = YtDlpExtractor::new(&fixture.executable);
    let destination = fixture.scratch.join("chosen.mkv");
    let reservation = reserved_destination(&destination);
    let bytes = backend
        .download(
            &ServiceDownloadRequest {
                url: "https://service.test/watch?v=abc".into(),
                format: ServiceFormatSelection::BestVideoAudio,
                destination: reservation,
                download_limit_kib: 0,
            },
            &CancelToken::new(),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(bytes, 7);
    assert_eq!(std::fs::read(destination).unwrap(), b"payload");
}
