/// Where the app keeps data on Linux, worked out one way everywhere.
library;

import 'dart:io';

/// `$XDG_DATA_HOME` when it is an absolute path, as the XDG rules require,
/// otherwise `~/.local/share`.
///
/// From the 13 November audit: three copies of this disagreed, and with
/// `XDG_DATA_HOME=""` one put history under `/orgbot/.history/` and the room
/// could not start.
String linuxDataHome([Map<String, String>? environment]) {
  final env = environment ?? Platform.environment;
  final xdg = env['XDG_DATA_HOME']?.trim();
  if (xdg != null && xdg.startsWith('/')) return xdg;
  return '${env['HOME'] ?? ''}/.local/share';
}
