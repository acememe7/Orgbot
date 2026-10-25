#include "native_workspace_panels.h"

#include <signal.h>
#include <fcntl.h>
#include <sys/types.h>
#include <unistd.h>

#include <cerrno>
#include <cmath>
#include <cstring>
#include <vector>

#include <vte/vte.h>
#include <webkit2/webkit2.h>

#include "flutter_linux/fl_standard_method_codec.h"
#include "native_browser_policy.h"

namespace {

struct Panels {
  GtkOverlay* surface;
  FlMethodChannel* channel;
  FlView* view;
  gchar* requested_browser_uri = nullptr;
  WebKitWebView* browser = nullptr;
  WebKitWebContext* browser_context = nullptr;
  GdkRectangle browser_geometry = {0, 0, 0, 0};
  VteTerminal* terminal = nullptr;
  GdkRectangle terminal_geometry = {0, 0, 0, 0};
  GPid terminal_pid = 0;
  bool terminal_spawn_pending = false;
  bool terminal_stop_requested = false;
  guint terminal_kill_source = 0;
  gchar* terminal_root = nullptr;
  gchar* pending_terminal_root = nullptr;
  gchar* media_directory = nullptr;
  gchar* media_base_uri = nullptr;
  gchar* media_key = nullptr;
  GCancellable* media_cancellable = nullptr;
  guint64 media_generation = 0;
  bool destroyed = false;
};

struct MediaPreviewContext {
  Panels* panels;
  FlMethodCall* call;
  guint64 generation;
  gchar* request_key;
  gchar* kind;
  gchar* extension;
  FlValue* bytes;
  gsize length;
};

struct MediaPreviewResult {
  gchar* directory;
};

void respond_error(FlMethodCall* call, const gchar* code,
                   const gchar* message);

void respond_error(FlMethodCall* call, const gchar* code,
                   const gchar* message) {
  fl_method_call_respond_error(call, code, message, nullptr, nullptr);
}

void hide_panel(Panels* panels, GtkWidget* child) {
  if (child == nullptr) return;
  GtkWidget* toplevel = gtk_widget_get_toplevel(GTK_WIDGET(panels->view));
  GtkWidget* focused = GTK_IS_WINDOW(toplevel)
                           ? gtk_window_get_focus(GTK_WINDOW(toplevel))
                           : nullptr;
  const bool panel_had_focus = focused != nullptr &&
                               (focused == child ||
                                gtk_widget_is_ancestor(focused, child));
  gtk_widget_hide(child);
  if (panel_had_focus) gtk_widget_grab_focus(GTK_WIDGET(panels->view));
}

gboolean position_overlay_child(GtkOverlay*, GtkWidget* child,
                                GdkRectangle* allocation, gpointer data) {
  auto* panels = static_cast<Panels*>(data);
  if (child == GTK_WIDGET(panels->browser)) {
    *allocation = panels->browser_geometry;
    return TRUE;
  }
  if (child == GTK_WIDGET(panels->terminal)) {
    *allocation = panels->terminal_geometry;
    return TRUE;
  }
  return FALSE;
}

const gchar* string_arg(FlValue* args, const gchar* key) {
  FlValue* value = fl_value_lookup_string(args, key);
  return value != nullptr && fl_value_get_type(value) == FL_VALUE_TYPE_STRING
             ? fl_value_get_string(value)
             : nullptr;
}

double number_arg(FlValue* args, const gchar* key, double fallback) {
  FlValue* value = fl_value_lookup_string(args, key);
  if (value == nullptr) return fallback;
  if (fl_value_get_type(value) == FL_VALUE_TYPE_INT) {
    return static_cast<double>(fl_value_get_int(value));
  }
  if (fl_value_get_type(value) == FL_VALUE_TYPE_FLOAT) {
    return fl_value_get_float(value);
  }
  return fallback;
}

bool set_geometry(Panels* panels, FlMethodCall* call, FlValue* args,
                  GtkWidget* child) {
  const double x = number_arg(args, "x", -1);
  const double y = number_arg(args, "y", -1);
  const double width = number_arg(args, "width", -1);
  const double height = number_arg(args, "height", -1);
  if (!std::isfinite(x) || !std::isfinite(y) || !std::isfinite(width) ||
      !std::isfinite(height) || x < 0 || y < 0 || width < 120 || height < 80 ||
      x > 10000 || y > 10000 || width > 10000 || height > 10000) {
    respond_error(call, "geometry", "The workspace panel size is invalid.");
    return false;
  }
  GdkRectangle geometry = {static_cast<gint>(std::floor(x)),
                           static_cast<gint>(std::floor(y)),
                           static_cast<gint>(std::ceil(width)),
                           static_cast<gint>(std::ceil(height))};
  if (child == GTK_WIDGET(panels->browser)) {
    panels->browser_geometry = geometry;
  } else if (child == GTK_WIDGET(panels->terminal)) {
    panels->terminal_geometry = geometry;
  } else {
    respond_error(call, "geometry", "The workspace panel is unavailable.");
    return false;
  }
  gtk_widget_queue_resize(GTK_WIDGET(panels->surface));
  return true;
}

gboolean browser_decide_policy(WebKitWebView*, WebKitPolicyDecision* decision,
                               WebKitPolicyDecisionType type, gpointer data) {
  if (type != WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION) {
    webkit_policy_decision_use(decision);
    return TRUE;
  }
  auto* navigation = WEBKIT_NAVIGATION_POLICY_DECISION(decision);
  WebKitURIRequest* request =
      webkit_navigation_action_get_request(
          webkit_navigation_policy_decision_get_navigation_action(navigation));
  const gchar* uri = request == nullptr ? nullptr : webkit_uri_request_get_uri(request);
  const auto* panels = static_cast<Panels*>(data);
  if (orgbot_allowed_browser_navigation(
          uri, panels == nullptr ? nullptr : panels->media_base_uri)) {
    webkit_policy_decision_use(decision);
  } else {
    webkit_policy_decision_ignore(decision);
  }
  return TRUE;
}

GtkWidget* browser_create_window(WebKitWebView*, WebKitNavigationAction*,
                                 gpointer) {
  return nullptr;
}

gboolean browser_load_failed(WebKitWebView* browser, WebKitLoadEvent,
                             const gchar*, GError* error, gpointer) {
  if (error != nullptr &&
      (g_error_matches(error, WEBKIT_NETWORK_ERROR,
                       WEBKIT_NETWORK_ERROR_CANCELLED) ||
       g_error_matches(error, G_IO_ERROR, G_IO_ERROR_CANCELLED))) {
    return FALSE;
  }
  webkit_web_view_load_html(
      browser,
      "<!doctype html><meta charset=utf-8><title>Preview unavailable</title>"
      "<main style='font:16px sans-serif;margin:2rem;color:#303936'>"
      "<h2>Page could not be loaded</h2><p>Check the address or network, then retry.</p></main>",
      nullptr);
  return TRUE;
}

void start_terminal(Panels* panels, const gchar* root);
void stop_terminal(Panels* panels);

void start_pending_terminal(Panels* panels) {
  if (panels->pending_terminal_root == nullptr || panels->terminal == nullptr) {
    return;
  }
  g_free(panels->terminal_root);
  panels->terminal_root = g_steal_pointer(&panels->pending_terminal_root);
  start_terminal(panels, panels->terminal_root);
}

void terminal_spawned(VteTerminal*, GPid pid, GError* error, gpointer data) {
  auto* view = FL_VIEW(data);
  auto* panels = static_cast<Panels*>(
      g_object_get_data(G_OBJECT(view), "orgbot-native-workspace-panels"));
  if (panels == nullptr) {
    if (pid > 0) {
      if (kill(-pid, SIGKILL) != 0) kill(pid, SIGKILL);
    }
    g_object_unref(view);
    return;
  }
  panels->terminal_spawn_pending = false;
  if (error != nullptr) {
    g_warning("Could not start local terminal: %s", error->message);
    panels->terminal_stop_requested = false;
    start_pending_terminal(panels);
    g_object_unref(view);
    return;
  }
  panels->terminal_pid = pid;
  if (panels->terminal_stop_requested) {
    stop_terminal(panels);
  }
  g_object_unref(view);
}

void terminal_child_exited(VteTerminal*, gint, gpointer data) {
  auto* panels = static_cast<Panels*>(data);
  if (panels->terminal_stop_requested && panels->terminal_pid > 0) {
    if (kill(-panels->terminal_pid, SIGKILL) != 0) {
      kill(panels->terminal_pid, SIGKILL);
    }
  }
  if (panels->terminal_kill_source != 0) {
    g_source_remove(panels->terminal_kill_source);
    panels->terminal_kill_source = 0;
  }
  panels->terminal_pid = 0;
  panels->terminal_spawn_pending = false;
  panels->terminal_stop_requested = false;
  start_pending_terminal(panels);
}

gchar** terminal_environment() {
  g_auto(GStrv) source = g_get_environ();
  std::vector<gchar*> kept;
  for (gchar** item = source; item != nullptr && *item != nullptr; ++item) {
    const gchar* separator = std::strchr(*item, '=');
    if (separator == nullptr) continue;
    g_autofree gchar* name = g_strndup(*item, separator - *item);
    if (g_str_has_prefix(name, "ORGBOT_") ||
        g_str_equal(name, "OPENAI_API_KEY") ||
        g_str_equal(name, "ANTHROPIC_API_KEY") ||
        g_str_equal(name, "DEEPSEEK_API_KEY") ||
        g_str_equal(name, "GROK_API_KEY") ||
        g_str_equal(name, "XAI_API_KEY") ||
        g_str_equal(name, "OLLAMA_API_KEY")) {
      continue;
    }
    kept.push_back(*item);
  }
  kept.push_back(nullptr);
  return g_strdupv(kept.data());
}

void start_terminal(Panels* panels, const gchar* root) {
  if (panels->terminal == nullptr || panels->terminal_pid != 0 ||
      panels->terminal_spawn_pending) {
    return;
  }
  g_autofree gchar* canonical = realpath(root, nullptr);
  if (canonical == nullptr || !g_file_test(canonical, G_FILE_TEST_IS_DIR)) {
    vte_terminal_feed(panels->terminal,
                     "Selected project folder is unavailable.\r\n", -1);
    return;
  }
  if (g_strcmp0(canonical, panels->terminal_root) != 0) {
    g_free(panels->terminal_root);
    panels->terminal_root = g_strdup(canonical);
  }
  gchar* argv[] = {const_cast<gchar*>("/bin/bash"),
                   const_cast<gchar*>("--noprofile"),
                   const_cast<gchar*>("--norc"),
                   const_cast<gchar*>("-i"), nullptr};
  g_auto(GStrv) environment = terminal_environment();
  panels->terminal_spawn_pending = true;
  panels->terminal_stop_requested = false;
  vte_terminal_spawn_async(panels->terminal, VTE_PTY_DEFAULT, canonical, argv,
                           environment, G_SPAWN_DEFAULT, nullptr, nullptr,
                           nullptr, 30000, nullptr, terminal_spawned,
                           g_object_ref(panels->view));
}

GtkWidget* ensure_browser(Panels* panels) {
  if (panels->browser != nullptr) return GTK_WIDGET(panels->browser);
  panels->browser_context = webkit_web_context_new_ephemeral();
  panels->browser = WEBKIT_WEB_VIEW(
      webkit_web_view_new_with_context(panels->browser_context));
  g_signal_connect(panels->browser, "decide-policy",
                   G_CALLBACK(browser_decide_policy), panels);
  g_signal_connect(panels->browser, "create",
                   G_CALLBACK(browser_create_window), panels);
  g_signal_connect(panels->browser, "load-failed",
                   G_CALLBACK(browser_load_failed), panels);
  gtk_widget_set_hexpand(GTK_WIDGET(panels->browser), TRUE);
  gtk_widget_set_vexpand(GTK_WIDGET(panels->browser), TRUE);
  gtk_widget_set_halign(GTK_WIDGET(panels->browser), GTK_ALIGN_FILL);
  gtk_widget_set_valign(GTK_WIDGET(panels->browser), GTK_ALIGN_FILL);
  gtk_overlay_add_overlay(panels->surface, GTK_WIDGET(panels->browser));
  return GTK_WIDGET(panels->browser);
}

GtkWidget* ensure_terminal(Panels* panels) {
  if (panels->terminal != nullptr) return GTK_WIDGET(panels->terminal);
  panels->terminal = VTE_TERMINAL(vte_terminal_new());
  vte_terminal_set_scrollback_lines(panels->terminal, 5000);
  vte_terminal_set_cursor_blink_mode(panels->terminal,
                                     VTE_CURSOR_BLINK_SYSTEM);
  g_autoptr(PangoFontDescription) font =
      pango_font_description_from_string("Monospace 10");
  vte_terminal_set_font(panels->terminal, font);
  GdkRGBA foreground;
  GdkRGBA background;
  GdkRGBA cursor;
  gdk_rgba_parse(&foreground, "#D7DFDA");
  gdk_rgba_parse(&background, "#202623");
  gdk_rgba_parse(&cursor, "#78B7AA");
  vte_terminal_set_color_foreground(panels->terminal, &foreground);
  vte_terminal_set_color_background(panels->terminal, &background);
  vte_terminal_set_color_cursor(panels->terminal, &cursor);
  g_signal_connect(panels->terminal, "child-exited",
                   G_CALLBACK(terminal_child_exited), panels);
  gtk_widget_set_hexpand(GTK_WIDGET(panels->terminal), TRUE);
  gtk_widget_set_vexpand(GTK_WIDGET(panels->terminal), TRUE);
  gtk_widget_set_halign(GTK_WIDGET(panels->terminal), GTK_ALIGN_FILL);
  gtk_widget_set_valign(GTK_WIDGET(panels->terminal), GTK_ALIGN_FILL);
  gtk_overlay_add_overlay(panels->surface, GTK_WIDGET(panels->terminal));
  return GTK_WIDGET(panels->terminal);
}

void stop_terminal(Panels* panels) {
  panels->terminal_stop_requested = true;
  const GPid pid = panels->terminal_pid;
  if (pid <= 0) return;
  if (kill(-pid, SIGTERM) != 0) kill(pid, SIGTERM);
  if (panels->terminal_kill_source == 0) {
    auto* saved_pid = new GPid(pid);
    panels->terminal_kill_source = g_timeout_add_seconds(
        2,
        [](gpointer data) -> gboolean {
          auto* state = static_cast<std::pair<Panels*, GPid*>*>(data);
          if (state->first->terminal_pid == *state->second) {
            if (kill(-*state->second, SIGKILL) != 0) kill(*state->second, SIGKILL);
          }
          state->first->terminal_kill_source = 0;
          delete state->second;
          delete state;
          return G_SOURCE_REMOVE;
        },
        new std::pair<Panels*, GPid*>(panels, saved_pid));
  }
}

void remove_media_directory(const gchar* path) {
  if (path == nullptr) return;
  g_autoptr(GFile) directory = g_file_new_for_path(path);
  g_autoptr(GFileEnumerator) children = g_file_enumerate_children(
      directory, G_FILE_ATTRIBUTE_STANDARD_NAME,
      G_FILE_QUERY_INFO_NOFOLLOW_SYMLINKS, nullptr, nullptr);
  if (children != nullptr) {
    while (true) {
      g_autoptr(GFileInfo) info =
          g_file_enumerator_next_file(children, nullptr, nullptr);
      if (info == nullptr) break;
      g_autoptr(GFile) child =
          g_file_get_child(directory, g_file_info_get_name(info));
      g_file_delete(child, nullptr, nullptr);
    }
  }
  g_file_delete(directory, nullptr, nullptr);
}

void remove_media(Panels* panels) {
  g_clear_pointer(&panels->media_key, g_free);
  g_clear_pointer(&panels->media_base_uri, g_free);
  if (panels->media_directory == nullptr) return;
  remove_media_directory(panels->media_directory);
  g_clear_pointer(&panels->media_directory, g_free);
}

void invalidate_media(Panels* panels, bool blank_browser) {
  ++panels->media_generation;
  if (panels->media_cancellable != nullptr) {
    g_cancellable_cancel(panels->media_cancellable);
    g_clear_object(&panels->media_cancellable);
  }
  if (blank_browser && panels->browser != nullptr) {
    webkit_web_view_load_html(panels->browser, "about:blank", nullptr);
  }
  remove_media(panels);
}

void cancel_pending_media(Panels* panels) {
  if (panels->media_cancellable == nullptr) return;
  ++panels->media_generation;
  g_cancellable_cancel(panels->media_cancellable);
  g_clear_object(&panels->media_cancellable);
}

void media_preview_context_free(gpointer data) {
  auto* context = static_cast<MediaPreviewContext*>(data);
  g_object_unref(context->call);
  g_free(context->request_key);
  g_free(context->kind);
  g_free(context->extension);
  fl_value_unref(context->bytes);
  delete context;
}

void media_preview_result_free(gpointer data) {
  auto* result = static_cast<MediaPreviewResult*>(data);
  remove_media_directory(result->directory);
  g_free(result->directory);
  delete result;
}

bool supported_media(const gchar* kind, const gchar* extension) {
  const gchar* video_extensions[] = {"mp4", "m4v", "webm", "ogv", "mov", "mkv", "avi", "mpeg", "mpg", "3gp", "flv"};
  const gchar* audio_extensions[] = {"mp3", "wav", "ogg", "oga", "flac", "m4a", "aac", "opus", "webm"};
  const bool video = g_strcmp0(kind, "video") == 0;
  const bool audio = g_strcmp0(kind, "audio") == 0;
  if (!video && !audio) return false;
  const gchar** extensions = video ? video_extensions : audio_extensions;
  const size_t count = video ? G_N_ELEMENTS(video_extensions)
                             : G_N_ELEMENTS(audio_extensions);
  for (size_t i = 0; i < count; ++i) {
    if (g_strcmp0(extension, extensions[i]) == 0) return true;
  }
  return false;
}

void write_media_preview_worker(GTask* task, gpointer, gpointer task_data,
                                GCancellable* cancellable) {
  auto* context = static_cast<MediaPreviewContext*>(task_data);
  if (g_cancellable_is_cancelled(cancellable)) {
    g_task_return_new_error(task, G_IO_ERROR, G_IO_ERROR_CANCELLED,
                            "Media preview request was cancelled.");
    return;
  }
  g_autoptr(GError) error = nullptr;
  g_autofree gchar* directory = g_dir_make_tmp("orgbot-preview-XXXXXX", &error);
  if (directory == nullptr) {
    g_task_return_new_error(task, G_IO_ERROR, G_IO_ERROR_FAILED,
                            "Could not create a private media preview.");
    return;
  }
  const bool video = g_str_equal(context->kind, "video");
  g_autofree gchar* filename =
      g_strdup_printf("%s.%s", video ? "video" : "audio", context->extension);
  g_autofree gchar* path = g_build_filename(directory, filename, nullptr);
  const int fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW,
                      0600);
  if (fd < 0) {
    remove_media_directory(directory);
    g_task_return_new_error(task, G_IO_ERROR, G_IO_ERROR_FAILED,
                            "Could not save a private media preview.");
    return;
  }
  gsize written = 0;
  bool saved = true;
  const guint8* bytes = fl_value_get_uint8_list(context->bytes);
  while (written < context->length) {
    if (g_cancellable_is_cancelled(cancellable)) {
      saved = false;
      break;
    }
    const ssize_t count = write(fd, bytes + written, context->length - written);
    if (count < 0 && errno == EINTR) continue;
    if (count <= 0) {
      saved = false;
      break;
    }
    written += static_cast<gsize>(count);
  }
  saved = saved && written == context->length && fsync(fd) == 0;
  close(fd);
  if (!saved) {
    remove_media_directory(directory);
    g_task_return_new_error(task, G_IO_ERROR,
                            g_cancellable_is_cancelled(cancellable)
                                ? G_IO_ERROR_CANCELLED
                                : G_IO_ERROR_FAILED,
                            "Could not save a private media preview.");
    return;
  }
  auto* result = new MediaPreviewResult{g_steal_pointer(&directory)};
  g_task_return_pointer(task, result, media_preview_result_free);
}

void media_preview_done(GObject*, GAsyncResult* async_result, gpointer) {
  auto* task = G_TASK(async_result);
  auto* context = static_cast<MediaPreviewContext*>(g_task_get_task_data(task));
  Panels* panels = context->panels;
  g_autoptr(GError) error = nullptr;
  auto* result = static_cast<MediaPreviewResult*>(
      g_task_propagate_pointer(task, &error));
  const bool current = !panels->destroyed &&
                       context->generation == panels->media_generation;
  if (!current) {
    if (result != nullptr) media_preview_result_free(result);
    respond_error(context->call, "media",
                  "The media preview changed before loading completed.");
    return;
  }
  g_clear_object(&panels->media_cancellable);
  if (result == nullptr) {
    respond_error(context->call, "media",
                  "Could not save a private media preview.");
    return;
  }
  panels->media_directory = g_steal_pointer(&result->directory);
  media_preview_result_free(result);
  g_autofree gchar* directory_uri =
      g_filename_to_uri(panels->media_directory, nullptr, nullptr);
  if (directory_uri == nullptr || panels->browser == nullptr) {
    remove_media(panels);
    respond_error(context->call, "media", "Could not open the media preview.");
    return;
  }
  const bool video = g_str_equal(context->kind, "video");
  WebKitSettings* settings = webkit_web_view_get_settings(panels->browser);
  // Keep WebKit's built-in media controls functional. The document is fixed
  // generated markup with a private media file; CSP prevents any page script
  // or external resource from running/loading.
  webkit_settings_set_enable_javascript(settings, TRUE);
  const gchar* tag = video
      ? "<video controls autoplay playsinline style='max-width:100%;max-height:100%'>"
      : "<audio controls autoplay style='width:100%'>";
  const gchar* close_tag = video ? "</video>" : "</audio>";
  g_autofree gchar* html = g_strdup_printf(
      "<!doctype html><meta charset=utf-8>"
      "<meta http-equiv='Content-Security-Policy' content=\"default-src 'none'; media-src 'self'; style-src 'unsafe-inline'; script-src 'none'\">"
      "<meta name=viewport content='width=device-width,initial-scale=1'>"
      "<body style='margin:0;background:#202623;display:grid;place-items:center;min-height:100vh'>%s"
      "<source src='%s.%s'>%s</body>",
      tag, video ? "video" : "audio", context->extension, close_tag);
  g_autofree gchar* base_uri = g_strconcat(directory_uri, "/", nullptr);
  g_free(panels->media_base_uri);
  panels->media_base_uri = g_strdup(base_uri);
  webkit_web_view_load_html(panels->browser, html, base_uri);
  g_free(panels->media_key);
  panels->media_key = g_strdup(context->request_key);
  gtk_widget_show(GTK_WIDGET(panels->browser));
  gtk_widget_grab_focus(GTK_WIDGET(panels->browser));
  fl_method_call_respond_success(context->call, fl_value_new_null(), nullptr);
}

bool write_media_preview(Panels* panels, FlValue* args, FlMethodCall* call,
                         const gchar* request_key) {
  FlValue* value = fl_value_lookup_string(args, "bytes");
  const gchar* kind = string_arg(args, "kind");
  const gchar* extension = string_arg(args, "extension");
  if (!supported_media(kind, extension) || value == nullptr ||
      fl_value_get_type(value) != FL_VALUE_TYPE_UINT8_LIST ||
      fl_value_get_length(value) == 0 ||
      fl_value_get_length(value) > 20 * 1024 * 1024) {
    respond_error(call, "media", "This media file cannot be previewed safely.");
    return false;
  }
  invalidate_media(panels, true);
  hide_panel(panels, GTK_WIDGET(panels->browser));
  panels->media_cancellable = g_cancellable_new();
  auto* context = new MediaPreviewContext{
      panels,
      FL_METHOD_CALL(g_object_ref(call)),
      panels->media_generation,
      g_strdup(request_key),
      g_strdup(kind),
      g_strdup(extension),
      fl_value_ref(value),
      fl_value_get_length(value),
  };
  GTask* task = g_task_new(G_OBJECT(panels->view), panels->media_cancellable,
                           media_preview_done, nullptr);
  g_task_set_task_data(task, context, media_preview_context_free);
  g_task_run_in_thread(task, write_media_preview_worker);
  g_object_unref(task);
  return true;
}

void respond_success(FlMethodCall* call) {
  fl_method_call_respond_success(call, fl_value_new_null(), nullptr);
}

void panel_method_call(FlMethodChannel*, FlMethodCall* call, gpointer data) {
  auto* panels = static_cast<Panels*>(data);
  const gchar* method = fl_method_call_get_name(call);
  FlValue* args = fl_method_call_get_args(call);
  if (g_str_has_prefix(method, "browser_")) {
    if (g_str_equal(method, "browser_hide")) {
      cancel_pending_media(panels);
      hide_panel(panels, GTK_WIDGET(panels->browser));
      respond_success(call);
      return;
    }
    if (g_str_equal(method, "browser_reload")) {
      if (panels->media_cancellable != nullptr &&
          panels->media_directory == nullptr) {
        invalidate_media(panels, false);
      }
      if (panels->browser != nullptr) webkit_web_view_reload(panels->browser);
      respond_success(call);
      return;
    }
    if (g_str_equal(method, "browser_media_hide")) {
      cancel_pending_media(panels);
      hide_panel(panels, GTK_WIDGET(panels->browser));
      respond_success(call);
      return;
    }
    if (g_str_equal(method, "browser_media_show")) {
      const gchar* kind = string_arg(args, "kind");
      const gchar* extension = string_arg(args, "extension");
      const gchar* media_key = string_arg(args, "media_key");
      if (media_key == nullptr || media_key[0] == '\0' ||
          std::strlen(media_key) > 8192) {
        respond_error(call, "media", "The media selection is invalid.");
        return;
      }
      if (!supported_media(kind, extension)) {
        respond_error(call, "media", "This media format cannot be previewed safely.");
        return;
      }
      GtkWidget* child = ensure_browser(panels);
      if (!set_geometry(panels, call, args, child)) return;
      g_autofree gchar* request_key =
          g_strdup_printf("%s|%s|%s", media_key, kind, extension);
      const bool reload = orgbot_media_request_needs_reload(
          panels->media_key, request_key, panels->media_directory != nullptr);
      if (reload) {
        write_media_preview(panels, args, call, request_key);
        return;
      }
      gtk_widget_show(child);
      gtk_widget_grab_focus(child);
      respond_success(call);
      return;
    }
    if (!g_str_equal(method, "browser_show")) {
      fl_method_call_respond_not_implemented(call, nullptr);
      return;
    }
    const gchar* uri = string_arg(args, "uri");
    if (!orgbot_safe_web_uri(uri)) {
      respond_error(call, "url", "Enter a valid HTTP or HTTPS address.");
      return;
    }
    GtkWidget* child = ensure_browser(panels);
    if (!set_geometry(panels, call, args, child)) return;
    const bool transitioning_from_media =
        panels->media_directory != nullptr ||
        panels->media_cancellable != nullptr;
    const bool needs_navigation = orgbot_browser_request_needs_navigation(
        panels->requested_browser_uri, uri, transitioning_from_media);
    if (transitioning_from_media) invalidate_media(panels, true);
    webkit_settings_set_enable_javascript(
        webkit_web_view_get_settings(panels->browser), TRUE);
    if (needs_navigation) {
      webkit_web_view_load_uri(panels->browser, uri);
      g_free(panels->requested_browser_uri);
      panels->requested_browser_uri = g_strdup(uri);
    }
    gtk_widget_show(child);
    gtk_widget_grab_focus(child);
    respond_success(call);
    return;
  }
  if (g_str_has_prefix(method, "terminal_")) {
    if (g_str_equal(method, "terminal_hide")) {
      hide_panel(panels, GTK_WIDGET(panels->terminal));
      respond_success(call);
      return;
    }
    if (g_str_equal(method, "terminal_stop")) {
      stop_terminal(panels);
      respond_success(call);
      return;
    }
    if (!g_str_equal(method, "terminal_show")) {
      fl_method_call_respond_not_implemented(call, nullptr);
      return;
    }
    const gchar* root = string_arg(args, "root");
    if (root == nullptr || root[0] != '/') {
      respond_error(call, "directory", "Choose a project folder first.");
      return;
    }
    GtkWidget* child = ensure_terminal(panels);
    if (!set_geometry(panels, call, args, child)) return;
    if (panels->terminal_root != nullptr &&
        g_strcmp0(root, panels->terminal_root) != 0) {
      if (panels->terminal_pid != 0 || panels->terminal_spawn_pending) {
        g_free(panels->pending_terminal_root);
        panels->pending_terminal_root = g_strdup(root);
        stop_terminal(panels);
      } else {
        g_free(panels->terminal_root);
        panels->terminal_root = g_strdup(root);
        vte_terminal_reset(panels->terminal, TRUE, TRUE);
      }
    }
    start_terminal(panels, root);
    gtk_widget_show(child);
    gtk_widget_grab_focus(child);
    respond_success(call);
    return;
  }
  fl_method_call_respond_not_implemented(call, nullptr);
}

void panels_free(gpointer data) {
  auto* panels = static_cast<Panels*>(data);
  panels->destroyed = true;
  invalidate_media(panels, false);
  g_clear_object(&panels->media_cancellable);
  stop_terminal(panels);
  if (panels->terminal_pid > 0) {
    if (kill(-panels->terminal_pid, SIGKILL) != 0) {
      kill(panels->terminal_pid, SIGKILL);
    }
  }
  if (panels->terminal_kill_source != 0) {
    g_source_remove(panels->terminal_kill_source);
  }
  if (panels->terminal != nullptr) {
    g_signal_handlers_disconnect_by_data(panels->terminal, panels);
  }
  remove_media(panels);
  g_free(panels->pending_terminal_root);
  g_clear_object(&panels->channel);
  g_clear_object(&panels->browser_context);
  g_free(panels->terminal_root);
  g_free(panels->requested_browser_uri);
  g_free(panels->media_base_uri);
  delete panels;
}

void panels_widget_destroyed(GtkWidget*, gpointer data) {
  auto* panels = static_cast<Panels*>(data);
  panels->destroyed = true;
  invalidate_media(panels, false);
}

}  // namespace

void native_workspace_panels_register(FlView* view, GtkOverlay* surface) {
  auto* messenger = fl_engine_get_binary_messenger(fl_view_get_engine(view));
  auto* panels = new Panels{surface, nullptr, view};
  g_signal_connect(surface, "get-child-position",
                   G_CALLBACK(position_overlay_child), panels);
  g_signal_connect(view, "destroy", G_CALLBACK(panels_widget_destroyed),
                   panels);
  g_autoptr(FlStandardMethodCodec) codec = fl_standard_method_codec_new();
  panels->channel = fl_method_channel_new(
      messenger, "orgbot/native_workspace_panels", FL_METHOD_CODEC(codec));
  fl_method_channel_set_method_call_handler(
      panels->channel, panel_method_call, panels, nullptr);
  g_object_set_data_full(G_OBJECT(view), "orgbot-native-workspace-panels",
                         panels, panels_free);
}
