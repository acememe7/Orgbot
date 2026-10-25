#include "desktop_files_channel.h"

#include <fcntl.h>
#include <limits.h>
#include <sys/stat.h>
#include <unistd.h>

#include <cstdlib>
#include <cstring>

#include "flutter_linux/fl_standard_method_codec.h"

namespace {

struct ChannelContext {
  GtkWindow* parent;
  FlMethodChannel* attachments;
  FlMethodChannel* workspace;
};

struct PickerContext {
  FlMethodCall* call;
  gboolean choose_directory;
};

struct PreviewContext {
  FlMethodCall* call;
  gchar* root;
  gchar* relative;
};

void respond_error(FlMethodCall* call, const gchar* code,
                   const gchar* message);

void preview_context_free(gpointer data) {
  auto* context = static_cast<PreviewContext*>(data);
  g_object_unref(context->call);
  g_free(context->root);
  g_free(context->relative);
  delete context;
}

void preview_worker(GTask* task, gpointer, gpointer task_data,
                    GCancellable*) {
  auto* context = static_cast<PreviewContext*>(task_data);
  g_autofree gchar* canonical_root = realpath(context->root, nullptr);
  if (canonical_root == nullptr) {
    g_task_return_new_error(task, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
                            "The selected project folder is unavailable.");
    return;
  }
  int directory_fd = open("/", O_RDONLY | O_CLOEXEC | O_DIRECTORY);
  if (directory_fd < 0) {
    g_task_return_new_error(task, G_IO_ERROR, g_io_error_from_errno(errno),
                            "Could not open the selected project folder.");
    return;
  }
  gchar** root_parts = g_strsplit(canonical_root + 1, "/", -1);
  gboolean failed = FALSE;
  for (gchar** part = root_parts; *part != nullptr && !failed; ++part) {
    if ((*part)[0] == '\0') continue;
    const int next = openat(directory_fd, *part,
                            O_RDONLY | O_CLOEXEC | O_DIRECTORY | O_NOFOLLOW);
    if (next < 0) {
      failed = TRUE;
    } else {
      close(directory_fd);
      directory_fd = next;
    }
  }
  g_strfreev(root_parts);
  gchar** parts = g_strsplit(context->relative, "/", -1);
  const gsize part_count = g_strv_length(parts);
  for (gsize index = 0; index < part_count && !failed; ++index) {
    if (parts[index][0] == '\0' || g_str_equal(parts[index], ".") ||
        g_str_equal(parts[index], "..")) {
      failed = TRUE;
      break;
    }
    const gboolean final = index + 1 == part_count;
    const int flags = O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK |
                      (final ? 0 : O_DIRECTORY);
    const int next = openat(directory_fd, parts[index], flags);
    if (next < 0) {
      failed = TRUE;
      break;
    }
    close(directory_fd);
    directory_fd = next;
  }
  g_strfreev(parts);
  struct stat metadata;
  if (failed || fstat(directory_fd, &metadata) != 0 ||
      !S_ISREG(metadata.st_mode) || metadata.st_size < 0 ||
      metadata.st_size > 20 * 1024 * 1024) {
    close(directory_fd);
    g_task_return_new_error(task, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                            "Only regular project files up to 20 MiB can be previewed.");
    return;
  }
  g_autofree guint8* bytes =
      static_cast<guint8*>(g_malloc(static_cast<gsize>(metadata.st_size)));
  gsize length = 0;
  while (length < static_cast<gsize>(metadata.st_size)) {
    const ssize_t count = read(directory_fd, bytes + length,
                               static_cast<gsize>(metadata.st_size) - length);
    if (count <= 0) {
      failed = TRUE;
      break;
    }
    length += static_cast<gsize>(count);
  }
  close(directory_fd);
  if (failed) {
    g_task_return_new_error(task, G_IO_ERROR, G_IO_ERROR_FAILED,
                            "Could not read the selected project file.");
    return;
  }
  g_task_return_pointer(task, g_bytes_new(bytes, length),
                        reinterpret_cast<GDestroyNotify>(g_bytes_unref));
}

void preview_done(GObject*, GAsyncResult* result, gpointer) {
  auto* task = G_TASK(result);
  auto* context = static_cast<PreviewContext*>(g_task_get_task_data(task));
  g_autoptr(GError) error = nullptr;
  GBytes* bytes = static_cast<GBytes*>(g_task_propagate_pointer(task, &error));
  if (bytes == nullptr) {
    respond_error(context->call, "file",
                  error == nullptr ? "Could not read the selected file."
                                   : error->message);
    return;
  }
  gsize length = 0;
  gconstpointer data = g_bytes_get_data(bytes, &length);
  g_autoptr(FlValue) value = fl_value_new_uint8_list(
      static_cast<const uint8_t*>(data), length);
  fl_method_call_respond_success(context->call, value, nullptr);
  g_bytes_unref(bytes);
}

void respond_error(FlMethodCall* call, const gchar* code,
                   const gchar* message) {
  fl_method_call_respond_error(call, code, message, nullptr, nullptr);
}

gboolean valid_room(const gchar* room) {
  if (room == nullptr || std::strlen(room) < 6 || std::strlen(room) > 102) {
    return FALSE;
  }
  for (const gchar* p = room; *p != '\0'; ++p) {
    if (!g_ascii_isalnum(*p) && *p != '_' && *p != '-') return FALSE;
  }
  return TRUE;
}

gboolean valid_attachment_id(const gchar* id) {
  if (id == nullptr || std::strlen(id) != 36 || std::strncmp(id, "att_", 4)) {
    return FALSE;
  }
  for (const gchar* p = id + 4; *p != '\0'; ++p) {
    if (!g_ascii_isdigit(*p) && (*p < 'a' || *p > 'f')) return FALSE;
  }
  return TRUE;
}

const gchar* argument_string(FlValue* arguments, const gchar* name) {
  FlValue* value = fl_value_lookup_string(arguments, name);
  return value != nullptr && fl_value_get_type(value) == FL_VALUE_TYPE_STRING
             ? fl_value_get_string(value)
             : nullptr;
}

gchar* attachment_directory(const gchar* room) {
  return g_build_filename(g_get_user_data_dir(), "orgbot", ".history", room,
                          "attachments", nullptr);
}

void picker_context_free(gpointer data, GClosure*) {
  auto* context = static_cast<PickerContext*>(data);
  g_object_unref(context->call);
  delete context;
}

void picker_response(GtkNativeDialog* dialog, gint response, gpointer data) {
  auto* context = static_cast<PickerContext*>(data);
  if (response != GTK_RESPONSE_ACCEPT) {
    g_autoptr(FlValue) result =
        context->choose_directory ? fl_value_new_null() : fl_value_new_list();
    fl_method_call_respond_success(context->call, result, nullptr);
    gtk_native_dialog_destroy(dialog);
    g_object_unref(dialog);
    return;
  }

  auto* chooser = GTK_FILE_CHOOSER(dialog);
  if (context->choose_directory) {
    g_autoptr(GFile) folder = gtk_file_chooser_get_file(chooser);
    g_autofree gchar* path = folder == nullptr ? nullptr : g_file_get_path(folder);
    if (path == nullptr) {
      respond_error(context->call, "directory", "Choose a local project folder.");
    } else {
      g_autoptr(FlValue) result = fl_value_new_string(path);
      fl_method_call_respond_success(context->call, result, nullptr);
    }
    gtk_native_dialog_destroy(dialog);
    g_object_unref(dialog);
    return;
  }

  g_autoptr(GError) error = nullptr;
  GSList* selected = gtk_file_chooser_get_files(chooser);
  const auto count = g_slist_length(selected);
  if (count > 5) {
    respond_error(context->call, "limit", "Choose up to five files at a time.");
    g_slist_free_full(selected, g_object_unref);
    gtk_native_dialog_destroy(dialog);
    g_object_unref(dialog);
    return;
  }

  g_autoptr(FlValue) result = fl_value_new_list();
  gboolean valid = TRUE;
  for (GSList* item = selected; item != nullptr; item = item->next) {
    auto* file = G_FILE(item->data);
    g_autoptr(GFileInfo) info = g_file_query_info(
        file, G_FILE_ATTRIBUTE_STANDARD_TYPE "," G_FILE_ATTRIBUTE_STANDARD_NAME
              "," G_FILE_ATTRIBUTE_STANDARD_CONTENT_TYPE ","
              G_FILE_ATTRIBUTE_STANDARD_SIZE,
        G_FILE_QUERY_INFO_NOFOLLOW_SYMLINKS, nullptr, &error);
    if (info == nullptr || g_file_info_get_file_type(info) != G_FILE_TYPE_REGULAR) {
      valid = FALSE;
      break;
    }
    if (g_file_info_get_size(info) < 0 ||
        g_file_info_get_size(info) > 20 * 1024 * 1024) {
      g_set_error_literal(&error, G_IO_ERROR, G_IO_ERROR_NO_SPACE,
                          "Each attachment must be 20 MiB or smaller.");
      valid = FALSE;
      break;
    }
    g_autofree gchar* path = g_file_get_path(file);
    if (path == nullptr) {
      g_set_error_literal(&error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                          "Choose files available on this computer.");
      valid = FALSE;
      break;
    }
    const gchar* name = g_file_info_get_name(info);
    const gchar* content_type = g_file_info_get_content_type(info);
    g_autofree gchar* mime = content_type == nullptr
                                 ? nullptr
                                 : g_content_type_get_mime_type(content_type);
    g_autoptr(FlValue) entry = fl_value_new_map();
    fl_value_set_string_take(entry, "path", fl_value_new_string(path));
    fl_value_set_string_take(entry, "name",
                             fl_value_new_string(name == nullptr ? "file" : name));
    fl_value_set_string_take(
        entry, "mime_type",
        fl_value_new_string(mime == nullptr ? "application/octet-stream" : mime));
    fl_value_append_take(result, g_steal_pointer(&entry));
  }
  g_slist_free_full(selected, g_object_unref);
  if (valid) {
    fl_method_call_respond_success(context->call, result, nullptr);
  } else {
    const gchar* message = error == nullptr ? "Choose regular local files only."
                                            : error->message;
    respond_error(context->call, "picker", message);
  }
  gtk_native_dialog_destroy(dialog);
  g_object_unref(dialog);
}

void show_picker(ChannelContext* context, FlMethodCall* call,
                 gboolean choose_directory) {
  GtkFileChooserAction action = choose_directory ? GTK_FILE_CHOOSER_ACTION_SELECT_FOLDER
                                                 : GTK_FILE_CHOOSER_ACTION_OPEN;
  GtkFileChooserNative* chooser = gtk_file_chooser_native_new(
      choose_directory ? "Choose project folder" : "Attach files",
      context->parent, action, choose_directory ? "Use folder" : "Attach",
      "Cancel");
  gtk_file_chooser_set_select_multiple(GTK_FILE_CHOOSER(chooser),
                                       !choose_directory);
  auto* picker = new PickerContext{FL_METHOD_CALL(g_object_ref(call)),
                                   choose_directory};
  g_signal_connect_data(chooser, "response", G_CALLBACK(picker_response),
                        picker, picker_context_free, G_CONNECT_DEFAULT);
  gtk_native_dialog_show(GTK_NATIVE_DIALOG(chooser));
}

void attachments_method_call(FlMethodChannel*, FlMethodCall* call,
                             gpointer user_data) {
  auto* context = static_cast<ChannelContext*>(user_data);
  const gchar* method = fl_method_call_get_name(call);
  FlValue* args = fl_method_call_get_args(call);
  if (g_strcmp0(method, "pick") == 0) {
    const gchar* room = argument_string(args, "room");
    if (!valid_room(room)) {
      respond_error(call, "room", "Invalid room identifier.");
      return;
    }
    show_picker(context, call, FALSE);
    return;
  }
  if (g_strcmp0(method, "open") == 0) {
    const gchar* room = argument_string(args, "room");
    const gchar* id = argument_string(args, "id");
    if (!valid_room(room) || !valid_attachment_id(id)) {
      respond_error(call, "attachment", "Invalid attachment reference.");
      return;
    }
    g_autofree gchar* root = attachment_directory(room);
    g_autofree gchar* path = g_build_filename(root, id, "original", nullptr);
    struct stat link_info;
    if (lstat(path, &link_info) != 0 || !S_ISREG(link_info.st_mode)) {
      respond_error(call, "attachment", "Attachment original is unavailable.");
      return;
    }
    const int fd = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK);
    struct stat file_info;
    if (fd < 0 || fstat(fd, &file_info) != 0 || !S_ISREG(file_info.st_mode)) {
      if (fd >= 0) close(fd);
      respond_error(call, "attachment", "Attachment is not a regular file.");
      return;
    }
    close(fd);
    g_autofree gchar* canonical_root = realpath(root, nullptr);
    g_autofree gchar* canonical_path = realpath(path, nullptr);
    if (canonical_root == nullptr || canonical_path == nullptr ||
        !g_str_has_prefix(canonical_path, canonical_root) ||
        canonical_path[std::strlen(canonical_root)] != G_DIR_SEPARATOR) {
      respond_error(call, "attachment", "Attachment leaves this room.");
      return;
    }
    g_autoptr(GFile) file = g_file_new_for_path(canonical_path);
    g_autofree gchar* uri = g_file_get_uri(file);
    g_autoptr(GError) error = nullptr;
    if (!g_app_info_launch_default_for_uri(uri, nullptr, &error)) {
      respond_error(call, "open", error == nullptr ? "No application can open this file."
                                                    : error->message);
      return;
    }
    fl_method_call_respond_success(call, fl_value_new_null(), nullptr);
    return;
  }
  fl_method_call_respond_not_implemented(call, nullptr);
}

void workspace_method_call(FlMethodChannel*, FlMethodCall* call,
                            gpointer user_data) {
  auto* context = static_cast<ChannelContext*>(user_data);
  if (g_strcmp0(fl_method_call_get_name(call), "present_window") == 0) {
    gtk_window_present(context->parent);
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }
  if (g_strcmp0(fl_method_call_get_name(call), "choose_directory") == 0) {
    show_picker(context, call, TRUE);
    return;
  }
  if (g_strcmp0(fl_method_call_get_name(call), "read_preview") == 0) {
    FlValue* arguments = fl_method_call_get_args(call);
    const gchar* root = argument_string(arguments, "root");
    const gchar* relative = argument_string(arguments, "path");
    if (root == nullptr || relative == nullptr || root[0] != '/' ||
        relative[0] == '/' || relative[0] == '\0') {
      respond_error(call, "path", "Choose a file inside the selected project.");
      return;
    }

    if (strstr(relative, "//") != nullptr || strstr(relative, "/./") != nullptr ||
        strstr(relative, "/../") != nullptr || g_str_has_suffix(relative, "/..")) {
      respond_error(call, "path", "Choose a file inside the selected project.");
      return;
    }
    auto* context = new PreviewContext{FL_METHOD_CALL(g_object_ref(call)),
                                       g_strdup(root), g_strdup(relative)};
    GTask* task = g_task_new(nullptr, nullptr, preview_done, nullptr);
    g_task_set_task_data(task, context, preview_context_free);
    g_task_run_in_thread(task, preview_worker);
    g_object_unref(task);
    return;
  }
  fl_method_call_respond_not_implemented(call, nullptr);
}

void channel_context_free(gpointer data) {
  auto* context = static_cast<ChannelContext*>(data);
  g_clear_object(&context->attachments);
  g_clear_object(&context->workspace);
  delete context;
}

}  // namespace

void desktop_files_channel_register(FlView* view) {
  auto* messenger = fl_engine_get_binary_messenger(fl_view_get_engine(view));
  auto* context = new ChannelContext{
      GTK_WINDOW(gtk_widget_get_toplevel(GTK_WIDGET(view))), nullptr, nullptr};
  g_autoptr(FlStandardMethodCodec) codec = fl_standard_method_codec_new();
  context->attachments = fl_method_channel_new(
      messenger, "orgbot/attachments", FL_METHOD_CODEC(codec));
  fl_method_channel_set_method_call_handler(
      context->attachments, attachments_method_call, context, nullptr);
  context->workspace = fl_method_channel_new(
      messenger, "orgbot/workspace", FL_METHOD_CODEC(codec));
  fl_method_channel_set_method_call_handler(
      context->workspace, workspace_method_call, context, nullptr);
  g_object_set_data_full(G_OBJECT(view), "orgbot-desktop-method-channels",
                         context, channel_context_free);
}
