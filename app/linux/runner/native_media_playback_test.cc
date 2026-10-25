#include <gtk/gtk.h>
#include <webkit2/webkit2.h>
#include <glib/gstdio.h>
#include <sys/stat.h>

#include <cstdio>

#include "native_browser_policy.h"

namespace {

struct EvalResult {
  GMainLoop* loop;
  gchar* value = nullptr;
  GError* error = nullptr;
};

void evaluate_finished(GObject* object, GAsyncResult* result, gpointer data) {
  auto* state = static_cast<EvalResult*>(data);
  JSCValue* value = webkit_web_view_evaluate_javascript_finish(
      WEBKIT_WEB_VIEW(object), result, &state->error);
  if (value != nullptr) {
    state->value = jsc_value_to_string(value);
    g_object_unref(value);
  }
  g_main_loop_quit(state->loop);
}

gchar* evaluate(WebKitWebView* view, GMainLoop* loop, const gchar* script) {
  EvalResult state{loop};
  webkit_web_view_evaluate_javascript(view, script, -1, nullptr, nullptr,
                                      nullptr, evaluate_finished, &state);
  g_main_loop_run(loop);
  if (state.error != nullptr) {
    g_printerr("WebKit evaluation failed: %s\n", state.error->message);
    g_error_free(state.error);
    g_clear_pointer(&state.value, g_free);
  }
  return state.value;
}

void wait_milliseconds(GMainLoop* loop, guint milliseconds) {
  g_timeout_add(milliseconds, [](gpointer data) -> gboolean {
    g_main_loop_quit(static_cast<GMainLoop*>(data));
    return G_SOURCE_REMOVE;
  }, loop);
  g_main_loop_run(loop);
}

struct LoadState {
  GMainLoop* loop;
  bool loaded = false;
};

void load_changed(WebKitWebView*, WebKitLoadEvent event, gpointer data) {
  if (event != WEBKIT_LOAD_FINISHED) return;
  auto* state = static_cast<LoadState*>(data);
  state->loaded = true;
  g_main_loop_quit(state->loop);
}

gboolean decide_policy(WebKitWebView*, WebKitPolicyDecision* decision,
                       WebKitPolicyDecisionType type, gpointer data) {
  if (type != WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION) {
    webkit_policy_decision_use(decision);
    return TRUE;
  }
  auto* request = webkit_navigation_action_get_request(
      webkit_navigation_policy_decision_get_navigation_action(
          WEBKIT_NAVIGATION_POLICY_DECISION(decision)));
  const gchar* uri = request == nullptr ? nullptr : webkit_uri_request_get_uri(request);
  auto* media_base_uri = static_cast<const gchar*>(data);
  if (orgbot_allowed_browser_navigation(uri, media_base_uri)) {
    webkit_policy_decision_use(decision);
  } else {
    webkit_policy_decision_ignore(decision);
  }
  return TRUE;
}

bool send_button_event(GtkWidget* widget, GdkEventType type, gint x, gint y) {
  GdkWindow* window = gtk_widget_get_window(widget);
  if (window == nullptr) return false;
  GdkEvent* event = gdk_event_new(type);
  event->button.window = GDK_WINDOW(g_object_ref(window));
  event->button.send_event = TRUE;
  event->button.time = GDK_CURRENT_TIME;
  event->button.x = x;
  event->button.y = y;
  event->button.button = 1;
  event->button.state = static_cast<GdkModifierType>(0);
  GdkDisplay* display = gdk_window_get_display(window);
  GdkSeat* seat = gdk_display_get_default_seat(display);
  GdkDevice* pointer = seat == nullptr ? nullptr : gdk_seat_get_pointer(seat);
  if (pointer == nullptr) {
    gdk_event_free(event);
    return false;
  }
  gdk_event_set_device(event, pointer);
  gdk_event_set_source_device(event, pointer);
  gtk_widget_event(widget, event);
  gdk_event_free(event);
  return true;
}

bool check_fixture(const gchar* fixture_path, bool video, bool javascript,
                   bool requires_gesture) {
  const gchar* filename = video ? "video.mp4" : "audio.wav";
  g_autofree gchar* directory = g_dir_make_tmp("orgbot-preview-XXXXXX", nullptr);
  g_autofree gchar* private_file = g_build_filename(directory, filename, nullptr);
  gchar* fixture_bytes = nullptr;
  gsize fixture_length = 0;
  if (!g_file_get_contents(fixture_path, &fixture_bytes, &fixture_length, nullptr) ||
      !g_file_set_contents(private_file, fixture_bytes, fixture_length, nullptr)) {
    g_free(fixture_bytes);
    g_printerr("Could not prepare private preview fixture\n");
    return false;
  }
  g_free(fixture_bytes);
  auto cleanup_fixture = [&]() {
    g_remove(private_file);
    g_rmdir(directory);
  };
  g_chmod(directory, 0700);
  g_chmod(private_file, 0600);
  g_autofree gchar* base_uri = g_filename_to_uri(directory, nullptr, nullptr);
  g_autofree gchar* base_uri_slash = g_strconcat(base_uri, "/", nullptr);
  g_autofree gchar* html = g_strdup_printf(
      "<!doctype html><meta http-equiv='Content-Security-Policy' content=\"default-src 'none'; media-src 'self'; style-src 'unsafe-inline'; script-src 'none'\">"
      "<meta name=viewport content='width=device-width,initial-scale=1'>"
      "<body style='margin:0;background:#202623;display:grid;place-items:center;min-height:100vh'>"
      "<script>window.orgbotInlineRan=true</script>"
      "<%s controls %s playsinline style='max-width:100%%;max-height:100%%'>"
      "<source src='%s'></%s></body>",
      video ? "video" : "audio", requires_gesture ? "" : "autoplay",
      filename, video ? "video" : "audio");
  GtkWidget* window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
  gtk_window_set_decorated(GTK_WINDOW(window), FALSE);
  gtk_window_set_skip_taskbar_hint(GTK_WINDOW(window), TRUE);
  gtk_window_move(GTK_WINDOW(window), -2000, -2000);
  gtk_window_set_default_size(GTK_WINDOW(window), 640, 420);
  WebKitWebContext* web_context = webkit_web_context_new_ephemeral();
  GtkWidget* view_widget = webkit_web_view_new_with_context(web_context);
  WebKitWebView* view = WEBKIT_WEB_VIEW(view_widget);
  g_object_unref(web_context);
  WebKitSettings* settings = webkit_web_view_get_settings(view);
  webkit_settings_set_enable_javascript(settings, javascript);
  if (requires_gesture) {
    webkit_settings_set_media_playback_requires_user_gesture(settings, TRUE);
  }
  gtk_container_add(GTK_CONTAINER(window), view_widget);
  g_signal_connect(view, "decide-policy", G_CALLBACK(decide_policy),
                   base_uri_slash);
  gtk_widget_show_all(window);

  GMainLoop* loop = g_main_loop_new(nullptr, FALSE);
  LoadState load{loop};
  g_signal_connect(view, "load-changed", G_CALLBACK(load_changed), &load);
  webkit_web_view_load_html(view, html, base_uri_slash);
  gtk_widget_show(view_widget);
  gtk_widget_grab_focus(view_widget);
  const guint load_timeout = g_timeout_add_seconds(15, [](gpointer data) -> gboolean {
    g_main_loop_quit(static_cast<GMainLoop*>(data));
    return G_SOURCE_REMOVE;
  }, loop);
  g_main_loop_run(loop);
  if (!load.loaded) {
    g_printerr("Timed out loading %s\n", filename);
    g_source_remove(load_timeout);
    gtk_widget_destroy(window);
    g_main_loop_unref(loop);
    cleanup_fixture();
    return false;
  }
  g_source_remove(load_timeout);

  GtkAllocation allocation;
  gtk_widget_get_allocation(view_widget, &allocation);
  const gint click_x = video ? 24 : 20;
  const gint click_y = video ? allocation.height - 18 : 26;
  const bool press_sent = send_button_event(view_widget, GDK_BUTTON_PRESS,
                                             click_x, click_y);
  const bool release_sent = send_button_event(view_widget, GDK_BUTTON_RELEASE,
                                               click_x, click_y);
  while (gtk_events_pending()) gtk_main_iteration();
  wait_milliseconds(loop, 2200);

  // Read playback state after the configured JavaScript interval. Enabling
  // script only for this QA read does not reload or change the media element.
  webkit_settings_set_enable_javascript(settings, TRUE);
  const gchar* element = video ? "video" : "audio";
  const bool effective_gesture =
      webkit_settings_get_media_playback_requires_user_gesture(settings);
  g_autofree gchar* script = g_strdup_printf(
      "(()=>{const m=document.querySelector('%s');return JSON.stringify({"
      "currentTime:m.currentTime,duration:m.duration,paused:m.paused,"
      "readyState:m.readyState,error:m.error?m.error.code:0,"
      "inlineScriptRan:!!window.orgbotInlineRan});})()", element);
  g_autofree gchar* result = evaluate(view, loop, script);
  const bool autoplay_progressed = result != nullptr &&
      g_strstr_len(result, -1, "\"currentTime\":0") == nullptr;
  if (!autoplay_progressed && !requires_gesture) {
    g_autofree gchar* play_script = g_strdup_printf(
        "document.querySelector('%s').play(); 'play-requested'", element);
    g_autofree gchar* play_result = evaluate(view, loop, play_script);
    wait_milliseconds(loop, 1000);
    g_clear_pointer(&result, g_free);
    result = evaluate(view, loop, script);
    g_print("  explicitPlay=%s\n",
            play_result == nullptr ? "unavailable" : play_result);
  }
  g_print("%s js=%s requiresGesture=%s click=%s autoplayProgress=%s status=%s\n",
          filename,
          javascript ? "on" : "off", effective_gesture ? "yes" : "no",
          press_sent && release_sent ? "sent" : "unavailable",
          autoplay_progressed ? "yes" : "no",
          result == nullptr ? "unavailable" : result);
  const bool metadata_ready = result != nullptr &&
                              !g_strstr_len(result, -1, "\"duration\":null");
  const bool inline_script_blocked = result != nullptr &&
      g_strstr_len(result, -1, "\"inlineScriptRan\":true") == nullptr;
  // This harness records results rather than treating the autoplay policy as
  // a product failure; autoplay is intentionally denied in gesture cases.
  gtk_widget_destroy(window);
  g_main_loop_unref(loop);
  cleanup_fixture();
  if (!metadata_ready || !inline_script_blocked) {
    g_printerr("CSP regression failed: local metadata readiness or inline-script blocking\n");
    return false;
  }
  return true;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 3) {
    g_printerr("Usage: %s sample.mp4 sample.wav\n", argv[0]);
    return 2;
  }
  gtk_init(&argc, &argv);
  const gchar* paths[] = {argv[1], argv[2]};
  for (const gchar* path : paths) {
    const bool video = g_str_has_suffix(path, ".mp4");
    if (!check_fixture(path, video, true, false) ||
        !check_fixture(path, video, false, false) ||
        !check_fixture(path, video, true, true) ||
        !check_fixture(path, video, false, true)) {
      return 1;
    }
  }
  return 0;
}
