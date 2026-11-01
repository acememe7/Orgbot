import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/protocol/models.dart';
import 'package:orgbot_app/ui/room_widgets.dart';

ChatMessage bot(String id, String agent, String body) => ChatMessage(
  id: id,
  author: Author(kind: 'agent', agentId: agent),
  body: body,
  intent: 'say',
  taskId: null,
  sentAt: const Timestamp(0),
);

void main() {
  test('consecutive short check-ins fold, and real news never does', () {
    // Wording from the live room at 00:58 to 01:05 on 1 November.
    final messages = [
      bot(
        'm1',
        'opus',
        'Aece, nothing needs you. Quinn\'s follow-up makes the freeze result firmer than Ellis\'s note suggested. '
            'Freeze margin: Quinn tested every possible change to a single run. No one bad run can push the gap past the 17 ms '
            'allowance; it would take two bad runs on the same build. The new build\'s worst frame was 100 ms.',
      ),
      bot(
        'm2',
        'finn',
        'I hold no task, so I am not starting work. The Halden freeze check is delivered, the gas proof is still with Mira.',
      ),
      bot(
        'm3',
        'astra',
        'Aece, no new review is ready. The findings are now in the gas proof criteria; I\'m leaving the waiting tasks alone.',
      ),
      bot(
        'm4',
        'mira',
        'Aece, I tightened the far-pose search: candidates must now be supported, outdoors and within bounds.',
      ),
    ];
    final runs = checkInRuns(messages);
    expect(
      runs.keys,
      unorderedEquals(['m2', 'm3']),
      reason: 'Julian\'s long message with a finding is not a check-in',
    );
    expect(runs['m2']!.start, 'm2');
    expect(runs['m3']!.members.map((m) => m.id), ['m2', 'm3']);
  });

  test('a single check-in stays as it is', () {
    final runs = checkInRuns([
      bot('a', 'finn', 'I hold no task, so I am not starting work.'),
      bot('b', 'mira', 'The capture reached gameplay.'),
    ]);
    expect(runs, isEmpty);
  });

  test('a short reply with a heading or a list is news, not a check-in', () {
    final runs = checkInRuns([
      bot(
        'a',
        'opus',
        'Aece, nothing needs you.\n\n**Freeze margin:** two bad runs would be needed.',
      ),
      bot('b', 'finn', 'I hold no task, so I am not starting work.'),
    ]);
    expect(runs, isEmpty);
  });

  test('real replies end with a newline and still count', () {
    // From the live room: every bot message body ended with a newline.
    final runs = checkInRuns([
      bot('a', 'finn', 'I hold no task, so I am not starting work.\n'),
      bot('b', 'astra', 'Aece, no new review is ready.\n'),
    ]);
    expect(runs.keys, ['a', 'b']);
  });
}
