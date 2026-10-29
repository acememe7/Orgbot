import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/net/room_client.dart';

/// A minimal room endpoint: admits every client and records what it sends.
class _Room {
  late final HttpServer server;
  final frames = <Map<String, Object?>>[];
  final sockets = <WebSocket>[];

  Future<void> start() async {
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      sockets.add(socket);
      socket.listen((data) {
        final frame = (jsonDecode(data as String) as Map).cast<String, Object?>();
        frames.add(frame);
        if (frame['type'] == 'hello') {
          socket.add(jsonEncode({
            'type': 'snapshot',
            'wire_version': frame['wire_version'],
            'roster': [],
            'tasks': [],
          }));
        }
      });
    });
  }

  String get url => 'ws://127.0.0.1:${server.port}/ws';

  Iterable<String> get said => frames
      .where((f) => f['type'] == 'send_message')
      .map((f) => f['body'] as String);
}

Future<void> _until(bool Function() done) async {
  for (var i = 0; i < 100 && !done(); i++) {
    await Future<void>.delayed(const Duration(milliseconds: 20));
  }
}

void main() {
  test('what the person says while disconnected is delivered on connect', () async {
    final room = _Room();
    await room.start();
    final client = RoomClient();
    addTearDown(() async {
      await client.close();
      await room.server.close(force: true);
    });

    client.send({'type': 'send_message', 'author': 'Aece', 'body': 'early idea'});
    await client.connect(room.url);
    await _until(() => room.said.isNotEmpty);
    expect(room.said, ['early idea']);

    // The room drops the connection; a message sent meanwhile waits for the
    // reconnect instead of vanishing.
    await room.sockets.single.close();
    await _until(() => client.current != LinkState.connected);
    client.send({'type': 'send_message', 'author': 'Aece', 'body': 'while dropped'});
    expect(room.said, ['early idea']);
    await client.connect(room.url);
    await _until(() => room.said.length == 2);
    expect(room.said, ['early idea', 'while dropped']);
  });
}
