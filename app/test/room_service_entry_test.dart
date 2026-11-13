// From 13 November 2026: the owner removed six finished rooms' folders to
// free disk space, and deleting such a room never stopped its service.
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/engine/core_server.dart';

void main() {
  test('a removed room is found by the unit that still names its folder', () async {
    final config = await Directory.systemTemp.createTemp('orgbot-units-');
    addTearDown(() => config.delete(recursive: true));
    await Directory('${config.path}/systemd/user').create(recursive: true);
    await File('${config.path}/systemd/user/orgbots-room-0123456789abcdef.service').writeAsString(
      '[Service]\nExecStart="/usr/lib/orgbot/orgbot-engine" "room-service-run" '
      '"--directory" "/data/orgbot/.services/room_10"\n',
    );
    final env = {'HOME': '/nowhere', 'XDG_CONFIG_HOME': config.path};
    expect(await hasRoomServiceEntry('/data/orgbot/.services/room_10', environment: env), isTrue);
    expect(
      await hasRoomServiceEntry('/data/orgbot/.services/room_1', environment: env),
      isFalse,
      reason: 'room_1 is not room_10',
    );
    expect(await hasRoomServiceEntry('/data/orgbot/.services/room_2', environment: env), isFalse);
  });
}
