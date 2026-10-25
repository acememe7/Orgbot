#include <gtk/gtk.h>
#include <webkit2/webkit2.h>

#include <cstdio>

namespace {

GdkRectangle expected = {48, 31, 360, 240};

gboolean get_child_position(GtkOverlay*, GtkWidget* child,
                            GdkRectangle* allocation, gpointer) {
  if (!WEBKIT_IS_WEB_VIEW(child)) return FALSE;
  *allocation = expected;
  return TRUE;
}

bool verify_allocation(GtkWidget* overlay, GtkWidget* browser,
                       const GdkRectangle& rectangle) {
  expected = rectangle;
  gtk_widget_queue_resize(overlay);
  gtk_container_check_resize(GTK_CONTAINER(overlay));
  while (gtk_events_pending()) gtk_main_iteration();

  GtkAllocation actual;
  gtk_widget_get_allocation(browser, &actual);
  gint actual_x = -1;
  gint actual_y = -1;
  const gboolean translated = gtk_widget_translate_coordinates(
      browser, overlay, 0, 0, &actual_x, &actual_y);
  if (translated && actual_x == rectangle.x && actual_y == rectangle.y &&
      actual.width == rectangle.width && actual.height == rectangle.height) {
    return true;
  }
  g_printerr("Expected WebKit allocation %d,%d %dx%d; got %d,%d %dx%d\n",
             rectangle.x, rectangle.y, rectangle.width, rectangle.height,
             translated ? actual_x : -1, translated ? actual_y : -1,
             actual.width, actual.height);
  return false;
}

}  // namespace

int main(int argc, char** argv) {
  gtk_init(&argc, &argv);
  GtkWidget* window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
  gtk_window_set_decorated(GTK_WINDOW(window), FALSE);
  gtk_window_set_skip_taskbar_hint(GTK_WINDOW(window), TRUE);
  gtk_window_move(GTK_WINDOW(window), -2000, -2000);
  gtk_window_set_default_size(GTK_WINDOW(window), 800, 600);
  GtkWidget* overlay = gtk_overlay_new();
  GtkWidget* base = gtk_drawing_area_new();
  GtkWidget* browser = webkit_web_view_new();
  gtk_widget_set_halign(browser, GTK_ALIGN_FILL);
  gtk_widget_set_valign(browser, GTK_ALIGN_FILL);
  gtk_container_add(GTK_CONTAINER(window), overlay);
  gtk_container_add(GTK_CONTAINER(overlay), base);
  gtk_overlay_add_overlay(GTK_OVERLAY(overlay), browser);
  g_signal_connect(overlay, "get-child-position",
                   G_CALLBACK(get_child_position), nullptr);
  gtk_widget_show_all(window);
  while (gtk_events_pending()) gtk_main_iteration();

  const bool normal = verify_allocation(
      overlay, browser, GdkRectangle{48, 31, 360, 240});
  const bool compact = verify_allocation(
      overlay, browser, GdkRectangle{12, 9, 140, 100});
  gtk_widget_destroy(window);
  return normal && compact ? 0 : 1;
}
