import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/engine/core_server.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/state/engine_room_controller.dart';

/// A room service that only counts what it was asked to do.
class FakeService implements CoreServer {
  int stops = 0;
  int detaches = 0;

  @override
  Future<void> stop() async => stops++;

  @override
  Future<void> detach() async => detaches++;

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

/// A room whose background service will not stop.
class StuckRoom extends EngineRoomController {
  StuckRoom({required super.room, required super.library});

  @override
  Future<void> stop() async => throw EngineStartError('The room\'s background service did not stop.');
}

void main() {
  // From the 13 November audit: a failed stop was swallowed, and Delete took
  // the room off the list while its service ran on.
  test('a room whose service will not stop stays, and says so', () async {
    final library = Library(saver: MemorySaver());
    await library.load();
    final room = RoomDef(name: 'Stuck', id: 'stuck_${DateTime.now().microsecondsSinceEpoch}', maxTurns: 0);
    library.rooms.add(room);
    final controller = StuckRoom(room: room, library: library);
    library.sessions[room.id] = controller;
    await expectLater(library.stopRoom(room.id), throwsA(isA<EngineStartError>()));
    expect(library.sessions.containsKey(room.id), isTrue);
    final problem = await library.removeRoom(room.id);
    expect(problem, contains('did not stop'));
    expect(library.rooms.any((r) => r.id == room.id), isTrue);
  });

  // From the 13 November audit: "Stop and save" or Delete while the room
  // was still starting found no service yet, and the one that came up a
  // second later ran on with no window attached.
  test('a service that comes up after the owner stopped the room is stopped', () async {
    final library = Library(saver: MemorySaver());
    await library.load();
    final room = RoomDef(name: 'Stopping', id: 'stop_${DateTime.now().microsecondsSinceEpoch}', maxTurns: 0);
    final controller = EngineRoomController(room: room, library: library);
    await controller.stop();
    final late = FakeService();
    expect(await controller.adoptStartedService(late), isFalse);
    expect(late.stops, 1);
    expect(late.detaches, 0);
    controller.dispose();
  });

  test('a service that comes up after only the screen closed keeps running', () async {
    final library = Library(saver: MemorySaver());
    await library.load();
    final room = RoomDef(name: 'Leaving', id: 'leave_${DateTime.now().microsecondsSinceEpoch}', maxTurns: 0);
    final controller = EngineRoomController(room: room, library: library);
    final fresh = FakeService();
    expect(await controller.adoptStartedService(fresh), isTrue);
    controller.dispose();
    final late = FakeService();
    expect(await controller.adoptStartedService(late), isFalse);
    expect(late.stops, 0);
    expect(late.detaches, 1);
  });
}
