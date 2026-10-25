#include "native_browser_policy.h"

#include <cassert>

int main() {
  constexpr const char* media_base = "file:///tmp/orgbot-preview-ab12cd/";
  assert(orgbot_allowed_browser_navigation(media_base, media_base));
  assert(orgbot_allowed_browser_navigation("about:blank", media_base));
  assert(orgbot_allowed_browser_navigation("https://example.test/page", nullptr));
  assert(!orgbot_allowed_browser_navigation(
      "file:///tmp/orgbot-preview-ab12cd/video.mp4", media_base));
  assert(!orgbot_allowed_browser_navigation(
      "file:///tmp/orgbot-preview-ab12cd-sibling/", media_base));
  assert(!orgbot_allowed_browser_navigation("file:///tmp/", media_base));
  assert(!orgbot_allowed_browser_navigation("javascript:alert(1)", media_base));
  assert(!orgbot_allowed_browser_navigation("https://user:pass@example.test/",
                                            media_base));
  assert(!orgbot_browser_request_needs_navigation(
      "http://127.0.0.1/fixture", "http://127.0.0.1/fixture", false));
  assert(orgbot_browser_request_needs_navigation(
      "http://127.0.0.1/fixture", "http://127.0.0.1/other", false));
  assert(orgbot_browser_request_needs_navigation(
      "http://127.0.0.1/fixture", "http://127.0.0.1/fixture", true));
  assert(!orgbot_media_request_needs_reload("same-key", "same-key", true));
  assert(orgbot_media_request_needs_reload("old-key", "new-key", true));
  return 0;
}
