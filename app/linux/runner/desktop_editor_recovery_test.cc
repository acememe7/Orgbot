// Standalone native regression: build this file with GTK/GLib flags and the
// Flutter Linux headers used by the runner, then run the resulting executable.
#include "desktop_editor_channel.cc"

#include <cstdio>
#include <string>

namespace {

constexpr char kTarget[] = "document.txt";
constexpr char kTemporary[] = ".orgbot-edit-recovery-test";
constexpr char kConcurrent[] = ".orgbot-edit-concurrent-test";
int g_directory_fd = -1;
bool g_raced = false;

void write_fixture(int directory_fd, const char* path, const char* content) {
  const int fd = openat(directory_fd, path,
                        O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
  g_assert_cmpint(fd, >=, 0);
  const gsize length = std::strlen(content);
  g_assert_true(write_all(fd, reinterpret_cast<const guint8*>(content), length));
  g_assert_cmpint(fsync(fd), ==, 0);
  close(fd);
}

std::string read_fixture(int directory_fd, const char* path) {
  const int fd = openat(directory_fd, path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
  g_assert_cmpint(fd, >=, 0);
  gchar bytes[128] = {};
  const ssize_t length = read(fd, bytes, sizeof(bytes));
  close(fd);
  g_assert_cmpint(length, >=, 0);
  return std::string(bytes, static_cast<size_t>(length));
}

int replace_during_exchange(int old_directory, const char* old_path,
                            int new_directory, const char* new_path,
                            unsigned int flags) {
  const int result = system_renameat2(old_directory, old_path, new_directory,
                                      new_path, flags);
  if (result != 0 || flags != RENAME_EXCHANGE || g_raced) return result;
  g_raced = true;

  // Make the swapped old inode fail its expected version token, then model a
  // concurrent editor installing a different file at the original path.
  const int old_fd = openat(g_directory_fd, kTemporary,
                            O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
  g_assert_cmpint(old_fd, >=, 0);
  struct stat metadata;
  g_assert_cmpint(fstat(old_fd, &metadata), ==, 0);
  struct timespec times[2] = {metadata.st_atim, metadata.st_mtim};
  times[1].tv_sec += 2;
  g_assert_cmpint(futimens(old_fd, times), ==, 0);
  close(old_fd);

  write_fixture(g_directory_fd, kConcurrent, "concurrent contents");
  g_assert_cmpint(renameat(g_directory_fd, kConcurrent, g_directory_fd,
                           kTarget), ==, 0);
  return result;
}

void test_failed_rollback_keeps_recovery_copy() {
  g_autofree gchar* directory = g_dir_make_tmp(
      "orgbot-editor-recovery-test-XXXXXX", nullptr);
  g_assert_nonnull(directory);
  g_directory_fd = open(directory, O_RDONLY | O_CLOEXEC | O_DIRECTORY);
  g_assert_cmpint(g_directory_fd, >=, 0);
  write_fixture(g_directory_fd, kTarget, "original contents");
  write_fixture(g_directory_fd, kTemporary, "staged edit");

  struct stat original;
  g_assert_cmpint(fstatat(g_directory_fd, kTarget, &original,
                          AT_SYMLINK_NOFOLLOW), ==, 0);
  const std::string expected = version_token(original);
  struct stat saved;
  g_autoptr(GError) error = nullptr;
  g_raced = false;
  const AtomicReplaceResult result = atomic_replace_with_cleanup(
      g_directory_fd, kTarget, kTemporary, expected.c_str(), 0600, &saved,
      &error, replace_during_exchange);

  g_assert_false(result.saved);
  g_assert_cmpint(static_cast<int>(result.temporary_state), ==,
                  static_cast<int>(TemporaryState::kRecoveryCopy));
  g_assert_nonnull(error);
  const std::string target_contents = read_fixture(g_directory_fd, kTarget);
  const std::string recovery_contents =
      read_fixture(g_directory_fd, kTemporary);
  g_assert_cmpstr(target_contents.c_str(), ==, "concurrent contents");
  g_assert_cmpstr(recovery_contents.c_str(), ==, "original contents");
  g_assert_true(g_raced);

  unlinkat(g_directory_fd, kTarget, 0);
  unlinkat(g_directory_fd, kTemporary, 0);
  close(g_directory_fd);
  g_directory_fd = -1;
  g_assert_cmpint(rmdir(directory), ==, 0);
}

}  // namespace

int main(int argc, char** argv) {
  g_test_init(&argc, &argv, nullptr);
  g_test_add_func("/editor/failed-rollback-preserves-recovery",
                  test_failed_rollback_keeps_recovery_copy);
  return g_test_run();
}
