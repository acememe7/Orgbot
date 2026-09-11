// Playbooks the bots can reach for.
//
// A bot backed by an agent CLI inherits that CLI's skills folder for free —
// it runs on a machine that has one. A bot backed by an API does not, and on
// a phone every bot is API-backed. So the whole roster on this device had no
// playbooks at all: twenty-one capable models each working out how to approach
// a problem from first principles, every time.
//
// These ship with the app and are unpacked to a folder the engine reads. The
// engine loads each skill's one-line description into view and fetches the
// body only when a bot asks for it, so carrying twenty costs a few hundred
// tokens and using none costs nothing.
library;

import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart' show rootBundle;
import 'package:path_provider/path_provider.dart';

/// Skills bundled with the app, by folder name.
///
/// Listed rather than discovered because Flutter's asset bundle has no
/// directory listing at runtime; the manifest is the only index, and reading
/// it to find six known files is more moving parts than naming them.
const List<String> bundledSkills = [
  'ponytail',
  'ponytail-audit',
  'ponytail-debt',
  'ponytail-gain',
  'ponytail-help',
  'ponytail-review',
];

/// Unpack the bundled playbooks and return the folder holding them.
///
/// Rewritten on every call rather than only when absent: a skill edited in a
/// new build of the app should take effect, and these are a few kilobytes.
/// Returns null if nothing could be written — a room without playbooks is
/// worse than one with them, but far better than a room that will not start.
Future<String?> unpackBundledSkills() async {
  try {
    final base = await getApplicationSupportDirectory();
    final root = Directory('${base.path}/skills');
    await root.create(recursive: true);
    for (final name in bundledSkills) {
      final text = await rootBundle.loadString('assets/skills/$name/SKILL.md');
      final dir = Directory('${root.path}/$name');
      await dir.create(recursive: true);
      await File('${dir.path}/SKILL.md').writeAsString(text);
    }
    return root.path;
  } catch (error) {
    debugPrint('could not unpack skills: $error');
    return null;
  }
}
