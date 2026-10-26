#include "my_application.h"

#include <flutter_linux/flutter_linux.h>
#ifdef GDK_WINDOWING_X11
#include <gdk/gdkx.h>
#endif

#include "flutter/generated_plugin_registrant.h"
#include "desktop_files_channel.h"
#include "desktop_editor_channel.h"
#include "native_workspace_panels.h"

struct _MyApplication {
  GtkApplication parent_instance;
  char** dart_entrypoint_arguments;
  FlMethodChannel* activation_channel;
  gchar* pending_activation_payload;
  gboolean activation_handler_ready;
};

G_DEFINE_TYPE(MyApplication, my_application, GTK_TYPE_APPLICATION)

static void my_application_deliver_activation(MyApplication* self,
                                               const gchar* payload) {
  if (self->activation_channel == nullptr || !self->activation_handler_ready) {
    g_free(self->pending_activation_payload);
    self->pending_activation_payload = g_strdup(payload);
    return;
  }
  g_autoptr(FlValue) value = fl_value_new_string(payload);
  fl_method_channel_invoke_method(self->activation_channel, "open_attention",
                                 value, nullptr, nullptr, nullptr);
}

static void my_application_activation_call(FlMethodChannel*, FlMethodCall* call,
                                           gpointer user_data) {
  auto* self = MY_APPLICATION(user_data);
  if (g_strcmp0(fl_method_call_get_name(call), "activation_ready") == 0) {
    self->activation_handler_ready = TRUE;
    if (self->pending_activation_payload != nullptr) {
      g_autofree gchar* payload = g_steal_pointer(&self->pending_activation_payload);
      my_application_deliver_activation(self, payload);
    }
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }
  fl_method_call_respond_not_implemented(call, nullptr);
}

// Called when first Flutter frame received.
static void first_frame_cb(MyApplication* self, FlView* view) {
  gtk_widget_show(gtk_widget_get_toplevel(GTK_WIDGET(view)));
}

static gboolean flutter_view_button_press_cb(GtkWidget* widget,
                                              GdkEventButton*, gpointer) {
  gtk_widget_grab_focus(widget);
  return FALSE;
}

// Point the window at the app's own icon.
//
// Flutter bundles anything declared under `assets:` into
// data/flutter_assets/ next to the executable, so the mark that ships with
// the app is reused here rather than installing a second copy. Without this
// the task bar and window switcher show a generic placeholder.
static void my_application_set_icon(GtkWindow* window) {
  g_autofree gchar* executable = g_file_read_link("/proc/self/exe", nullptr);
  if (executable == nullptr) {
    return;
  }
  g_autofree gchar* directory = g_path_get_dirname(executable);
  g_autofree gchar* icon = g_build_filename(
      directory, "data", "flutter_assets", "assets", "orgbots-desktop.png", nullptr);
  if (g_file_test(icon, G_FILE_TEST_EXISTS)) {
    gtk_window_set_icon_from_file(window, icon, nullptr);
  }
}

// Implements GApplication::activate.
static void my_application_activate(GApplication* application) {
  MyApplication* self = MY_APPLICATION(application);
  GtkWindow* existing = gtk_application_get_active_window(GTK_APPLICATION(application));
  if (existing == nullptr) {
    // The first window is not active until GTK presents the first Flutter
    // frame. A notification can arrive during that interval, so reuse the
    // registered application window instead of creating a second engine.
    GList* windows = gtk_application_get_windows(GTK_APPLICATION(application));
    if (windows != nullptr) {
      existing = GTK_WINDOW(windows->data);
    }
  }
  if (existing != nullptr) {
    gtk_window_present(existing);
    return;
  }
  GtkWindow* window =
      GTK_WINDOW(gtk_application_window_new(GTK_APPLICATION(application)));

  // Use a header bar when running in GNOME as this is the common style used
  // by applications and is the setup most users will be using (e.g. Ubuntu
  // desktop).
  // If running on X and not using GNOME then just use a traditional title bar
  // in case the window manager does more exotic layout, e.g. tiling.
  // If running on Wayland assume the header bar will work (may need changing
  // if future cases occur).
  gboolean use_header_bar = TRUE;
#ifdef GDK_WINDOWING_X11
  GdkScreen* screen = gtk_window_get_screen(window);
  if (GDK_IS_X11_SCREEN(screen)) {
    const gchar* wm_name = gdk_x11_screen_get_window_manager_name(screen);
    if (g_strcmp0(wm_name, "GNOME Shell") != 0) {
      use_header_bar = FALSE;
    }
  }
#endif
  if (use_header_bar) {
    GtkHeaderBar* header_bar = GTK_HEADER_BAR(gtk_header_bar_new());
    gtk_widget_show(GTK_WIDGET(header_bar));
    gtk_header_bar_set_title(header_bar, "OrgBots");
    gtk_header_bar_set_show_close_button(header_bar, TRUE);
    gtk_window_set_titlebar(window, GTK_WIDGET(header_bar));
  } else {
    gtk_window_set_title(window, "OrgBots");
  }

  my_application_set_icon(window);
  gtk_window_set_default_size(window, 1280, 720);

  g_autoptr(FlDartProject) project = fl_dart_project_new();
  fl_dart_project_set_dart_entrypoint_arguments(
      project, self->dart_entrypoint_arguments);
  // Keep Dart off the GTK main thread. By default Flutter runs the UI isolate
  // on the platform thread, so whenever GTK is busy presenting the previous
  // frame (on GNOME's Xorg session with NVIDIA that wait is one or two
  // refreshes), the next frame cannot even start, and the app drew at 15-19
  // fps however little each frame cost. On its own thread, Dart keeps
  // producing frames while GTK presents. ORGBOT_UI_ON_PLATFORM_THREAD=1
  // restores the default.
  if (g_strcmp0(g_getenv("ORGBOT_UI_ON_PLATFORM_THREAD"), "1") != 0) {
    fl_dart_project_set_ui_thread_policy(
        project, FL_UI_THREAD_POLICY_RUN_ON_SEPARATE_THREAD);
  }

  FlView* view = fl_view_new(project);
  gtk_widget_add_events(GTK_WIDGET(view), GDK_BUTTON_PRESS_MASK);
  g_signal_connect(view, "button-press-event",
                   G_CALLBACK(flutter_view_button_press_cb), nullptr);
  GdkRGBA background_color;
  // Background defaults to black, override it here if necessary, e.g. #00000000
  // for transparent.
  gdk_rgba_parse(&background_color, "#000000");
  fl_view_set_background_color(view, &background_color);
  gtk_widget_show(GTK_WIDGET(view));
  GtkOverlay* overlay = GTK_OVERLAY(gtk_overlay_new());
  gtk_container_add(GTK_CONTAINER(overlay), GTK_WIDGET(view));
  gtk_container_add(GTK_CONTAINER(window), GTK_WIDGET(overlay));
  gtk_widget_show(GTK_WIDGET(overlay));

  // Show the window when Flutter renders.
  // Requires the view to be realized so we can start rendering.
  g_signal_connect_swapped(view, "first-frame", G_CALLBACK(first_frame_cb),
                           self);
  gtk_widget_realize(GTK_WIDGET(view));

  g_autoptr(FlStandardMethodCodec) activation_codec = fl_standard_method_codec_new();
  g_clear_object(&self->activation_channel);
  self->activation_channel = fl_method_channel_new(
      fl_engine_get_binary_messenger(fl_view_get_engine(view)),
      "orgbot/activation", FL_METHOD_CODEC(activation_codec));
  self->activation_handler_ready = FALSE;
  fl_method_channel_set_method_call_handler(
      self->activation_channel, my_application_activation_call, self, nullptr);

  fl_register_plugins(FL_PLUGIN_REGISTRY(view));
  desktop_files_channel_register(view);
  desktop_editor_channel_register(view);
  native_workspace_panels_register(view, overlay);

  gtk_widget_grab_focus(GTK_WIDGET(view));
}

// Forward notification actions to an existing window instead of a second app.
static gint my_application_command_line(GApplication* application,
                                        GApplicationCommandLine* command_line) {
  MyApplication* self = MY_APPLICATION(application);
  gint count = 0;
  g_auto(GStrv) arguments = g_application_command_line_get_arguments(command_line, &count);
  if (self->activation_channel != nullptr) {
    for (gint i = 1; i + 1 < count; ++i) {
      if (g_strcmp0(arguments[i], "--orgbot-attention") == 0) {
        my_application_deliver_activation(self, arguments[i + 1]);
        break;
      }
    }
  } else {
    g_clear_pointer(&self->dart_entrypoint_arguments, g_strfreev);
    self->dart_entrypoint_arguments = g_strdupv(arguments + 1);
  }
  g_application_activate(application);
  return 0;
}

// Implements GApplication::startup.
static void my_application_startup(GApplication* application) {
  // MyApplication* self = MY_APPLICATION(object);

  // Perform any actions required at application startup.

  G_APPLICATION_CLASS(my_application_parent_class)->startup(application);
}

// Implements GApplication::shutdown.
static void my_application_shutdown(GApplication* application) {
  // MyApplication* self = MY_APPLICATION(object);

  // Perform any actions required at application shutdown.

  G_APPLICATION_CLASS(my_application_parent_class)->shutdown(application);
}

// Implements GObject::dispose.
static void my_application_dispose(GObject* object) {
  MyApplication* self = MY_APPLICATION(object);
  g_clear_pointer(&self->dart_entrypoint_arguments, g_strfreev);
  g_clear_pointer(&self->pending_activation_payload, g_free);
  g_clear_object(&self->activation_channel);
  G_OBJECT_CLASS(my_application_parent_class)->dispose(object);
}

static void my_application_class_init(MyApplicationClass* klass) {
  G_APPLICATION_CLASS(klass)->activate = my_application_activate;
  G_APPLICATION_CLASS(klass)->command_line = my_application_command_line;
  G_APPLICATION_CLASS(klass)->startup = my_application_startup;
  G_APPLICATION_CLASS(klass)->shutdown = my_application_shutdown;
  G_OBJECT_CLASS(klass)->dispose = my_application_dispose;
}

static void my_application_init(MyApplication* self) {}

MyApplication* my_application_new() {
  // Set the program name to the application ID, which helps various systems
  // like GTK and desktop environments map this running application to its
  // corresponding .desktop file. This ensures better integration by allowing
  // the application to be recognized beyond its binary name.
  g_set_prgname(APPLICATION_ID);

  return MY_APPLICATION(g_object_new(my_application_get_type(),
                                     "application-id", APPLICATION_ID, "flags",
                                     G_APPLICATION_HANDLES_COMMAND_LINE, nullptr));
}
