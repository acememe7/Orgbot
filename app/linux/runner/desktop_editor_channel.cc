#include <fcntl.h>
#include <linux/fs.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <cerrno>
#include <cstring>
#include <string>
#include <utility>
#include <vector>

#include "flutter_linux/flutter_linux.h"
#include "flutter_linux/fl_standard_method_codec.h"

namespace {

constexpr gsize kMaxEditableBytes = 2 * 1024 * 1024;

struct EditorContext {
  FlMethodCall* call;
  gchar* root;
  gchar* relative;
  gchar* expected_token;
  guint8* bytes;
  gsize length;
  gboolean write;
};

void respond_error(FlMethodCall* call, const gchar* message) {
  fl_method_call_respond_error(call, "workspace_file", message, nullptr,
                               nullptr);
}

const gchar* string_arg(FlValue* args, const gchar* name) {
  FlValue* value = fl_value_lookup_string(args, name);
  return value != nullptr && fl_value_get_type(value) == FL_VALUE_TYPE_STRING
             ? fl_value_get_string(value)
             : nullptr;
}

bool valid_relative_path(const gchar* path) {
  if (path == nullptr || path[0] == '\0' || path[0] == '/') return false;
  gchar** parts = g_strsplit(path, "/", -1);
  bool valid = true;
  for (gchar** part = parts; *part != nullptr; ++part) {
    if ((*part)[0] == '\0' || g_str_equal(*part, ".") ||
        g_str_equal(*part, "..")) {
      valid = false;
      break;
    }
  }
  g_strfreev(parts);
  return valid;
}

int open_root_directory(const gchar* root) {
  g_autofree gchar* canonical = realpath(root, nullptr);
  if (canonical == nullptr || canonical[0] != '/') return -1;
  int directory = open("/", O_RDONLY | O_CLOEXEC | O_DIRECTORY);
  if (directory < 0) return -1;
  gchar** parts = g_strsplit(canonical + 1, "/", -1);
  for (gchar** part = parts; *part != nullptr; ++part) {
    if ((*part)[0] == '\0') continue;
    const int next = openat(directory, *part,
                            O_RDONLY | O_CLOEXEC | O_DIRECTORY | O_NOFOLLOW);
    if (next < 0) {
      close(directory);
      directory = -1;
      break;
    }
    close(directory);
    directory = next;
  }
  g_strfreev(parts);
  return directory;
}

int open_parent_directory(int root_fd, const gchar* relative,
                          gchar** basename) {
  gchar** parts = g_strsplit(relative, "/", -1);
  const gsize count = g_strv_length(parts);
  if (count == 0) {
    g_strfreev(parts);
    return -1;
  }
  int directory = root_fd;
  for (gsize index = 0; index + 1 < count; ++index) {
    const int next = openat(directory, parts[index],
                            O_RDONLY | O_CLOEXEC | O_DIRECTORY | O_NOFOLLOW);
    if (next < 0) {
      close(directory);
      directory = -1;
      break;
    }
    close(directory);
    directory = next;
  }
  if (directory >= 0) {
    *basename = g_strdup(parts[count - 1]);
  }
  g_strfreev(parts);
  return directory;
}

std::string version_token(const struct stat& metadata) {
  return std::to_string(static_cast<unsigned long long>(metadata.st_dev)) + ":" +
         std::to_string(static_cast<unsigned long long>(metadata.st_ino)) + ":" +
         std::to_string(static_cast<long long>(metadata.st_size)) + ":" +
         std::to_string(static_cast<long long>(metadata.st_mtim.tv_sec)) + ":" +
         std::to_string(static_cast<long long>(metadata.st_mtim.tv_nsec));
}

bool same_file(const struct stat& left, const struct stat& right) {
  return left.st_dev == right.st_dev && left.st_ino == right.st_ino;
}

void respond_document(FlMethodCall* call, const guint8* bytes, gsize length,
                      const struct stat& metadata) {
  const std::string token = version_token(metadata);
  g_autoptr(FlValue) response = fl_value_new_map();
  g_autoptr(FlValue) content = fl_value_new_uint8_list(bytes, length);
  g_autoptr(FlValue) token_value = fl_value_new_string(token.c_str());
  fl_value_set_string_take(response, "bytes", g_steal_pointer(&content));
  fl_value_set_string_take(response, "token", g_steal_pointer(&token_value));
  fl_method_call_respond_success(call, response, nullptr);
}

void editor_context_free(gpointer data) {
  auto* context = static_cast<EditorContext*>(data);
  g_object_unref(context->call);
  g_free(context->root);
  g_free(context->relative);
  g_free(context->expected_token);
  g_free(context->bytes);
  delete context;
}

void read_worker(GTask* task, gpointer, gpointer task_data, GCancellable*) {
  auto* context = static_cast<EditorContext*>(task_data);
  int parent_fd = open_root_directory(context->root);
  g_autofree gchar* basename = nullptr;
  if (parent_fd < 0 ||
      (parent_fd = open_parent_directory(parent_fd, context->relative,
                                         &basename)) < 0) {
    g_task_return_new_error(task, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
                            "The selected project file is unavailable.");
    return;
  }
  const int file_fd = openat(parent_fd, basename,
                             O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK);
  struct stat before;
  if (file_fd < 0 || fstat(file_fd, &before) != 0 || !S_ISREG(before.st_mode) ||
      before.st_size < 0 || static_cast<gsize>(before.st_size) > kMaxEditableBytes) {
    if (file_fd >= 0) close(file_fd);
    close(parent_fd);
    g_task_return_new_error(task, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                            "Only regular text files up to 2 MiB can be edited.");
    return;
  }
  g_autofree guint8* content =
      static_cast<guint8*>(g_malloc(static_cast<gsize>(before.st_size)));
  gsize offset = 0;
  bool failed = false;
  while (offset < static_cast<gsize>(before.st_size)) {
    const ssize_t read_count =
        read(file_fd, content + offset, static_cast<gsize>(before.st_size) - offset);
    if (read_count <= 0) {
      failed = true;
      break;
    }
    offset += static_cast<gsize>(read_count);
  }
  struct stat after;
  if (fstat(file_fd, &after) != 0 || version_token(before) != version_token(after)) {
    failed = true;
  }
  close(file_fd);
  close(parent_fd);
  if (failed) {
    g_task_return_new_error(task, G_IO_ERROR, G_IO_ERROR_BUSY,
                            "The file changed while it was being opened. Reload it.");
    return;
  }
  auto* result = new std::pair<guint8*, struct stat>(content, after);
  content = nullptr;
  g_task_return_pointer(task, result, [](gpointer value) {
    auto* result = static_cast<std::pair<guint8*, struct stat>*>(value);
    g_free(result->first);
    delete result;
  });
}

void read_done(GObject*, GAsyncResult* result, gpointer) {
  auto* task = G_TASK(result);
  auto* context = static_cast<EditorContext*>(g_task_get_task_data(task));
  g_autoptr(GError) error = nullptr;
  auto* output = static_cast<std::pair<guint8*, struct stat>*>(
      g_task_propagate_pointer(task, &error));
  if (output == nullptr) {
    respond_error(context->call,
                  error == nullptr ? "Could not read the project file."
                                   : error->message);
    return;
  }
  respond_document(context->call, output->first,
                   static_cast<gsize>(output->second.st_size), output->second);
  g_free(output->first);
  delete output;
}

bool write_all(int fd, const guint8* bytes, gsize length) {
  gsize offset = 0;
  while (offset < length) {
    const ssize_t count = write(fd, bytes + offset, length - offset);
    if (count < 0 && errno == EINTR) continue;
    if (count <= 0) return false;
    offset += static_cast<gsize>(count);
  }
  return true;
}

enum class TemporaryState {
  kCallerOwned,
  kConsumed,
  kRecoveryCopy,
};

struct AtomicReplaceResult {
  bool saved;
  TemporaryState temporary_state;
};

using RenameAt2Operation = int (*)(int, const char*, int, const char*,
                                   unsigned int);

int system_renameat2(int old_directory, const char* old_path,
                     int new_directory, const char* new_path,
                     unsigned int flags) {
#ifdef SYS_renameat2
  return syscall(SYS_renameat2, old_directory, old_path, new_directory,
                 new_path, flags);
#else
  errno = ENOSYS;
  return -1;
#endif
}

AtomicReplaceResult atomic_replace(
    int directory_fd, const gchar* basename, const gchar* temporary,
    const gchar* expected, mode_t mode, struct stat* result, GError** error,
    RenameAt2Operation rename_operation = system_renameat2) {
  TemporaryState temporary_state = TemporaryState::kCallerOwned;
  struct stat existing;
  const gboolean existed = fstatat(directory_fd, basename, &existing,
                                  AT_SYMLINK_NOFOLLOW) == 0;
  if (!existed && errno != ENOENT) {
    g_set_error_literal(error, G_IO_ERROR, g_io_error_from_errno(errno),
                        "Could not inspect the current project file.");
    return {false, temporary_state};
  }
  if (expected == nullptr || g_strcmp0(expected, "new") == 0) {
    if (existed) {
      g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_EXISTS,
                          "A file already exists at this path. Reload before saving.");
      return {false, temporary_state};
    }
#ifdef SYS_renameat2
    if (rename_operation(directory_fd, temporary, directory_fd, basename,
                         RENAME_NOREPLACE) != 0) {
      g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_EXISTS,
                          "A file appeared at this path. Reload before saving.");
      return {false, temporary_state};
    }
    temporary_state = TemporaryState::kConsumed;
#else
    if (linkat(directory_fd, temporary, directory_fd, basename, 0) != 0 ||
        unlinkat(directory_fd, temporary, 0) != 0) {
      g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_FAILED,
                          "Could not safely create the project file.");
      return {false, temporary_state};
    }
    temporary_state = TemporaryState::kConsumed;
#endif
  } else {
    if (!existed || !S_ISREG(existing.st_mode) ||
        version_token(existing) != expected) {
      g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_BUSY,
                          "The file changed since it was opened. Reload before saving.");
      return {false, temporary_state};
    }
    struct stat new_file;
    if (fstatat(directory_fd, temporary, &new_file, AT_SYMLINK_NOFOLLOW) != 0) {
      g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_FAILED,
                          "Could not verify the temporary edit.");
      return {false, temporary_state};
    }
#ifdef SYS_renameat2
    if (rename_operation(directory_fd, temporary, directory_fd, basename,
                         RENAME_EXCHANGE) != 0) {
      g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_FAILED,
                          "Atomic file replacement is unavailable on this filesystem.");
      return {false, temporary_state};
    }
    struct stat swapped;
    const bool expected_file =
        fstatat(directory_fd, temporary, &swapped, AT_SYMLINK_NOFOLLOW) == 0 &&
        version_token(swapped) == expected;
    if (!expected_file) {
      struct stat current;
      const bool new_file_still_at_target =
          fstatat(directory_fd, basename, &current, AT_SYMLINK_NOFOLLOW) == 0 &&
          same_file(current, new_file);
      const bool restored = new_file_still_at_target &&
          rename_operation(directory_fd, temporary, directory_fd, basename,
                           RENAME_EXCHANGE) == 0;
      if (restored) {
        if (unlinkat(directory_fd, temporary, 0) == 0) {
          temporary_state = TemporaryState::kConsumed;
        }
      } else {
        temporary_state = TemporaryState::kRecoveryCopy;
      }
      g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_BUSY,
                          restored
                              ? "The file changed during save. Reload before saving."
                              : "A concurrent file update could not be restored; its recovery copy was preserved beside the file.");
      return {false, temporary_state};
    }
    if (unlinkat(directory_fd, temporary, 0) != 0) {
      g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_FAILED,
                          "The file was saved, but its backup could not be removed.");
      return {false, temporary_state};
    }
    temporary_state = TemporaryState::kConsumed;
#else
    g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                        "Atomic conflict-safe file replacement is unavailable.");
    return {false, temporary_state};
#endif
  }
  const int saved_fd = openat(directory_fd, basename,
                              O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK);
  const bool saved = saved_fd >= 0 && fstat(saved_fd, result) == 0 &&
                     S_ISREG(result->st_mode);
  if (saved_fd >= 0) close(saved_fd);
  if (!saved) {
    g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_FAILED,
                        "The saved file could not be verified.");
    return {false, temporary_state};
  }
  (void)mode;
  return {true, temporary_state};
}

AtomicReplaceResult atomic_replace_with_cleanup(
    int directory_fd, const gchar* basename, const gchar* temporary,
    const gchar* expected, mode_t mode, struct stat* result, GError** error,
    RenameAt2Operation rename_operation = system_renameat2) {
  const AtomicReplaceResult replacement = atomic_replace(
      directory_fd, basename, temporary, expected, mode, result, error,
      rename_operation);
  if (replacement.temporary_state == TemporaryState::kCallerOwned) {
    unlinkat(directory_fd, temporary, 0);
  }
  return replacement;
}

void write_worker(GTask* task, gpointer, gpointer task_data, GCancellable*) {
  auto* context = static_cast<EditorContext*>(task_data);
  int root_fd = open_root_directory(context->root);
  g_autofree gchar* basename = nullptr;
  if (root_fd < 0) {
    g_task_return_new_error(task, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
                            "The selected project folder is unavailable.");
    return;
  }
  const int parent_fd = open_parent_directory(root_fd, context->relative,
                                               &basename);
  if (parent_fd < 0) {
    g_task_return_new_error(task, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
                            "The selected project folder is unavailable.");
    return;
  }
  if (context->length > kMaxEditableBytes) {
    close(parent_fd);
    g_task_return_new_error(task, G_IO_ERROR, G_IO_ERROR_NO_SPACE,
                            "Edited files are limited to 2 MiB.");
    return;
  }
  struct stat old_file;
  const bool exists = fstatat(parent_fd, basename, &old_file,
                              AT_SYMLINK_NOFOLLOW) == 0;
  if ((!exists && errno != ENOENT) ||
      (exists && !S_ISREG(old_file.st_mode)) ||
      (exists && context->expected_token != nullptr &&
       version_token(old_file) != context->expected_token) ||
      (!exists && g_strcmp0(context->expected_token, "new") != 0)) {
    close(parent_fd);
    g_task_return_new_error(task, G_IO_ERROR, G_IO_ERROR_BUSY,
                            "The file changed since it was opened. Reload before saving.");
    return;
  }
  const mode_t mode = exists ? old_file.st_mode & 0777 : 0600;
  g_autofree gchar* temporary = nullptr;
  int temp_fd = -1;
  for (int attempt = 0; attempt < 8 && temp_fd < 0; ++attempt) {
    g_free(temporary);
    temporary = g_strdup_printf(".orgbot-edit-%ld-%08x", static_cast<long>(getpid()),
                                g_random_int());
    temp_fd = openat(parent_fd, temporary,
                     O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW,
                     mode);
  }
  if (temp_fd < 0) {
    close(parent_fd);
    g_task_return_new_error(task, G_IO_ERROR, g_io_error_from_errno(errno),
                            "Could not create a safe temporary file.");
    return;
  }
  const bool written = fchmod(temp_fd, mode) == 0 &&
                       write_all(temp_fd, context->bytes, context->length) &&
                       fsync(temp_fd) == 0;
  close(temp_fd);
  if (!written) {
    unlinkat(parent_fd, temporary, 0);
    close(parent_fd);
    g_task_return_new_error(task, G_IO_ERROR, G_IO_ERROR_FAILED,
                            "Could not write the temporary edit.");
    return;
  }
  g_autoptr(GError) error = nullptr;
  struct stat saved;
  const AtomicReplaceResult replacement = atomic_replace_with_cleanup(
      parent_fd, basename, temporary, context->expected_token, mode, &saved,
      &error);
  close(parent_fd);
  if (!replacement.saved) {
    g_task_return_error(task, g_steal_pointer(&error));
    return;
  }
  auto* result = new struct stat(saved);
  g_task_return_pointer(task, result, g_free);
}

void write_done(GObject*, GAsyncResult* result, gpointer) {
  auto* task = G_TASK(result);
  auto* context = static_cast<EditorContext*>(g_task_get_task_data(task));
  g_autoptr(GError) error = nullptr;
  auto* metadata = static_cast<struct stat*>(g_task_propagate_pointer(task, &error));
  if (metadata == nullptr) {
    respond_error(context->call,
                  error == nullptr ? "Could not save the project file."
                                   : error->message);
    return;
  }
  const std::string token = version_token(*metadata);
  g_autoptr(FlValue) value = fl_value_new_string(token.c_str());
  fl_method_call_respond_success(context->call, value, nullptr);
  g_free(metadata);
}

void editor_method_call(FlMethodChannel*, FlMethodCall* call, gpointer) {
  const gchar* method = fl_method_call_get_name(call);
  FlValue* args = fl_method_call_get_args(call);
  const gchar* root = string_arg(args, "root");
  const gchar* relative = string_arg(args, "path");
  if (root == nullptr || root[0] != '/' || !valid_relative_path(relative)) {
    respond_error(call, "Choose a file inside the selected project.");
    return;
  }
  auto* context = new EditorContext{FL_METHOD_CALL(g_object_ref(call)),
                                    g_strdup(root), g_strdup(relative), nullptr,
                                    nullptr, 0, FALSE};
  GAsyncReadyCallback completion = nullptr;
  GTaskThreadFunc worker = nullptr;
  if (g_str_equal(method, "read")) {
    worker = read_worker;
    completion = read_done;
  } else if (g_str_equal(method, "save")) {
    FlValue* bytes = fl_value_lookup_string(args, "bytes");
    const gchar* expected = string_arg(args, "expected_token");
    if (bytes == nullptr || fl_value_get_type(bytes) != FL_VALUE_TYPE_UINT8_LIST ||
        expected == nullptr || std::strlen(expected) > 256) {
      editor_context_free(context);
      respond_error(call, "The edit request is incomplete.");
      return;
    }
    context->expected_token = g_strdup(expected);
    context->length = fl_value_get_length(bytes);
    context->bytes = static_cast<guint8*>(g_memdup2(
        fl_value_get_uint8_list(bytes), context->length));
    context->write = TRUE;
    worker = write_worker;
    completion = write_done;
  } else {
    editor_context_free(context);
    fl_method_call_respond_not_implemented(call, nullptr);
    return;
  }
  GTask* task = g_task_new(nullptr, nullptr, completion, nullptr);
  g_task_set_task_data(task, context, editor_context_free);
  g_task_run_in_thread(task, worker);
  g_object_unref(task);
}

}  // namespace

void desktop_editor_channel_register(FlView* view) {
  auto* messenger = fl_engine_get_binary_messenger(fl_view_get_engine(view));
  g_autoptr(FlStandardMethodCodec) codec = fl_standard_method_codec_new();
  g_autoptr(FlMethodChannel) channel = fl_method_channel_new(
      messenger, "orgbot/workspace_editor", FL_METHOD_CODEC(codec));
  fl_method_channel_set_method_call_handler(channel, editor_method_call, nullptr,
                                            nullptr);
  g_object_set_data_full(G_OBJECT(view), "orgbot-workspace-editor-channel",
                         g_object_ref(channel), g_object_unref);
}
