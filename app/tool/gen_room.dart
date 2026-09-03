// Write a room file from sample bots, to check the engine accepts what the
// app generates. Run: dart run tool/gen_room.dart <out.toml>
import 'dart:io';

import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/orgbot/room_file.dart';

void main(List<String> args) {
  final out = args.isEmpty ? '/tmp/rfgen/room.toml' : args.first;
  final bots = [
    BotDef(name: 'Lead', handle: 'lead', provider: ProviderKind.anthropic, role: 'coordinator'),
    BotDef(name: 'Dev', handle: 'dev', provider: ProviderKind.codex, role: 'engineer'),
    BotDef(name: 'Quill', handle: 'quill', provider: ProviderKind.grok, role: 'reviewer'),
  ];
  final room = RoomDef(
      name: 'Release notes',
      goal: 'Write the 1.3 release notes',
      path: '/tmp',
      tasks: ['Draft the notes']);
  final file = buildRoomFile(room, bots);
  File(out)
    ..createSync(recursive: true)
    ..writeAsStringSync(file.toml);
  stdout.writeln('wrote $out; skipped: ${file.skipped}');
}
