#ifndef ORGBOT_NATIVE_BROWSER_POLICY_H_
#define ORGBOT_NATIVE_BROWSER_POLICY_H_

#include <glib.h>

#include <cstring>

inline bool orgbot_safe_web_uri(const gchar* value) {
  if (value == nullptr || std::strlen(value) > 4096) return false;
  g_autoptr(GError) error = nullptr;
  g_autoptr(GUri) uri = g_uri_parse(value, G_URI_FLAGS_NONE, &error);
  if (uri == nullptr) return false;
  const gchar* scheme = g_uri_get_scheme(uri);
  const gchar* host = g_uri_get_host(uri);
  return (g_strcmp0(scheme, "http") == 0 ||
          g_strcmp0(scheme, "https") == 0) &&
         host != nullptr && host[0] != '\0' && g_uri_get_userinfo(uri) == nullptr;
}

inline bool orgbot_allowed_browser_navigation(const gchar* value,
                                               const gchar* media_base_uri) {
  if (orgbot_safe_web_uri(value)) return true;
  if (value == nullptr) return false;
  return g_strcmp0(value, "about:blank") == 0 ||
         (media_base_uri != nullptr &&
          g_strcmp0(value, media_base_uri) == 0);
}

inline bool orgbot_browser_request_needs_navigation(
    const gchar* last_requested_uri, const gchar* requested_uri,
    bool transitioning_from_media) {
  return transitioning_from_media || last_requested_uri == nullptr ||
         g_strcmp0(last_requested_uri, requested_uri) != 0;
}

inline bool orgbot_media_request_needs_reload(const gchar* current_key,
                                              const gchar* requested_key,
                                              bool has_directory) {
  return !has_directory || current_key == nullptr || requested_key == nullptr ||
         g_strcmp0(current_key, requested_key) != 0;
}

#endif  // ORGBOT_NATIVE_BROWSER_POLICY_H_
