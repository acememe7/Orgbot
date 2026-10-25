#include "native_browser_policy.h"

#include <gtk/gtk.h>
#include <webkit2/webkit2.h>

namespace {

struct AsyncState {
  GMainLoop* loop;
  gchar* value = nullptr;
  GError* error = nullptr;
};

guint finished_loads = 0;
GMainLoop* load_loop = nullptr;
GdkRectangle browser_geometry = {24, 18, 360, 240};

gboolean position_browser(GtkOverlay*, GtkWidget* child,
                          GdkRectangle* allocation, gpointer) {
  if (!WEBKIT_IS_WEB_VIEW(child)) return FALSE;
  *allocation = browser_geometry;
  return TRUE;
}

void update_geometry(GtkWidget* overlay, const GdkRectangle& geometry) {
  browser_geometry = geometry;
  gtk_widget_queue_resize(overlay);
  gtk_container_check_resize(GTK_CONTAINER(overlay));
  while (gtk_events_pending()) gtk_main_iteration();
}

void load_changed(WebKitWebView*, WebKitLoadEvent event, gpointer) {
  if (event != WEBKIT_LOAD_FINISHED) return;
  ++finished_loads;
  if (load_loop != nullptr) g_main_loop_quit(load_loop);
}

void javascript_finished(GObject* source, GAsyncResult* result,
                         gpointer data) {
  auto* state = static_cast<AsyncState*>(data);
  JSCValue* value = webkit_web_view_evaluate_javascript_finish(
      WEBKIT_WEB_VIEW(source), result, &state->error);
  if (value != nullptr) {
    state->value = jsc_value_to_string(value);
    g_object_unref(value);
  }
  g_main_loop_quit(state->loop);
}

gchar* run_javascript(WebKitWebView* browser, GMainLoop* loop,
                      const gchar* source) {
  AsyncState state{loop};
  webkit_web_view_evaluate_javascript(browser, source, -1, nullptr, nullptr,
                                      nullptr, javascript_finished, &state);
  g_main_loop_run(loop);
  if (state.error != nullptr) {
    g_printerr("JavaScript evaluation failed: %s\n", state.error->message);
    g_error_free(state.error);
    g_clear_pointer(&state.value, g_free);
  }
  return state.value;
}

}  // namespace

int main(int argc, char** argv) {
  gtk_init(&argc, &argv);
  GtkWidget* window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
  gtk_window_set_decorated(GTK_WINDOW(window), FALSE);
  gtk_window_set_skip_taskbar_hint(GTK_WINDOW(window), TRUE);
  gtk_window_move(GTK_WINDOW(window), -2000, -2000);
  gtk_window_set_default_size(GTK_WINDOW(window), 640, 420);
  GtkWidget* overlay = gtk_overlay_new();
  GtkWidget* browser = webkit_web_view_new();
  gtk_container_add(GTK_CONTAINER(window), overlay);
  gtk_container_add(GTK_CONTAINER(overlay), gtk_drawing_area_new());
  gtk_overlay_add_overlay(GTK_OVERLAY(overlay), browser);
  gtk_widget_set_halign(browser, GTK_ALIGN_FILL);
  gtk_widget_set_valign(browser, GTK_ALIGN_FILL);
  g_signal_connect(overlay, "get-child-position", G_CALLBACK(position_browser),
                   nullptr);
  gtk_widget_show_all(window);

  GMainLoop* loop = g_main_loop_new(nullptr, FALSE);
  load_loop = loop;
  g_signal_connect(browser, "load-changed", G_CALLBACK(load_changed), nullptr);
  webkit_web_view_load_html(
      WEBKIT_WEB_VIEW(browser),
      "<!doctype html><input id='state' value='initial'>",
      "http://127.0.0.1/lifecycle-fixture");
  const guint load_timeout = g_timeout_add_seconds(15, [](gpointer data) -> gboolean {
    g_main_loop_quit(static_cast<GMainLoop*>(data));
    return G_SOURCE_REMOVE;
  }, loop);
  g_main_loop_run(loop);
  g_source_remove(load_timeout);
  if (finished_loads != 1) {
    g_printerr("Expected one initial page load, observed %u\n", finished_loads);
    gtk_widget_destroy(window);
    g_main_loop_unref(loop);
    return 1;
  }

  gchar* changed = run_javascript(WEBKIT_WEB_VIEW(browser), loop,
      "document.getElementById('state').value='retained'");
  if (changed == nullptr || g_strcmp0(changed, "retained") != 0) {
    g_printerr("Could not set local page state\n");
    g_free(changed);
    gtk_widget_destroy(window);
    g_main_loop_unref(loop);
    return 1;
  }
  g_free(changed);

  const gchar* requested_uri = "http://127.0.0.1/lifecycle-fixture";
  update_geometry(overlay, GdkRectangle{48, 31, 420, 260});
  const bool resize_navigates = orgbot_browser_request_needs_navigation(
      requested_uri, requested_uri, false);
  gtk_widget_hide(browser);
  update_geometry(overlay, GdkRectangle{18, 12, 300, 180});
  gtk_widget_show(browser);
  const bool reopen_navigates = orgbot_browser_request_needs_navigation(
      requested_uri, requested_uri, false);
  if (resize_navigates || reopen_navigates) {
    webkit_web_view_load_uri(WEBKIT_WEB_VIEW(browser), requested_uri);
  }
  while (gtk_events_pending()) gtk_main_iteration();

  gchar* retained = run_javascript(WEBKIT_WEB_VIEW(browser), loop,
      "document.getElementById('state').value");
  const bool state_preserved = g_strcmp0(retained, "retained") == 0;
  g_free(retained);
  const bool no_extra_load = finished_loads == 1;
  g_main_loop_unref(loop);
  gtk_widget_destroy(window);
  if (!state_preserved || !no_extra_load) {
    g_printerr("Page state/load count changed across resize/reopen\n");
    return 1;
  }
  return 0;
}
