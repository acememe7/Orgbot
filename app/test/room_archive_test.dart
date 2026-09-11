// A run log that stops is a room that appears to have gone quiet.
//
// The cap itself was right: one unbounded file is slow to open and impossible
// to prune. Stopping at it was not. A four-day run reached twenty megabytes
// and the transcript simply ended — no more chat, no more events — while the
// board carried on saving perfectly well. The note in the code said segmented
// logs could wait until a real run reached the cap. One did.


import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/state/room_archive.dart';

void main() {
  Map<String, Object?> frame(String filler) => {
        'type': 'event',
        'envelope': {'seq': 1, 'body': filler},
      };

  test('a run log rolls to a new segment rather than stopping', () async {
    final room = 'roll_${DateTime.now().microsecondsSinceEpoch}';
    final problems = <String>[];
    // A small cap, so the boundary is reachable without writing 20 MiB.
    final archive = await RoomArchive.open(
      room,
      problems.add,
      segmentBytes: 4096,
    );

    for (var i = 0; i < 40; i++) {
      archive.append([frame('x' * 400)]);
    }
    await archive.close();

    expect(
      problems,
      isEmpty,
      reason: 'rolling is ordinary behaviour, not something to report',
    );
    expect(
      archive.segments.length,
      greaterThan(1),
      reason: 'past the cap the log continues in a new segment',
    );
    for (final segment in archive.segments) {
      expect(await segment.exists(), isTrue);
    }

    // And every segment is offered to the reader, newest first.
    final listed = await RoomArchive.list(room);
    expect(listed.length, archive.segments.length);

    for (final segment in archive.segments) {
      await segment.delete();
    }
  });

  test('a short run stays in one file', () async {
    final room = 'short_${DateTime.now().microsecondsSinceEpoch}';
    final archive = await RoomArchive.open(room, (_) {}, segmentBytes: 4096);
    archive.append([frame('small')]);
    await archive.close();

    expect(archive.segments.length, 1, reason: 'nothing to roll yet');
    for (final segment in archive.segments) {
      await segment.delete();
    }
  });
}
