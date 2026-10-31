import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/ui/leftover_copies_dialog.dart';
import 'package:orgbot_app/ui/workspace_changes_dialog.dart';

void main() {
  testWidgets('lists leftover copies and discards one after confirming', (
    tester,
  ) async {
    final calls = <List<String>>[];
    var discarded = false;
    await tester.pumpWidget(
      _host(
        runner: (executable, arguments) async {
          calls.add(arguments);
          if (arguments[1] == 'discard') {
            discarded = true;
            return _result({
              'ok': true,
              'agent': 'bot_old',
              'rescued': 2,
              'history': true,
              'rescue_dir': '/home/me/.local/share/orgbot/leftovers/bot_old-1',
              'freed_bytes': 3200000000,
            });
          }
          return _result({
            'ok': true,
            'dry_run': true,
            'removed': [
              {'agent': 'bot_done', 'bytes': 2000000},
            ],
            'kept': [
              if (!discarded)
                {
                  'agent': 'bot_old',
                  'reason': 'its baseline or project could not be read',
                  'bytes': 3200000000,
                },
            ],
            'freed_bytes': 0,
          });
        },
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();

    expect(calls.single.sublist(1, 3), ['prune', '--dry-run']);
    expect(
      calls.single,
      containsAll(['--keep', 'bot_live', 'bot_live-verify']),
    );
    expect(
      find.text('3.2 GB · its baseline or project could not be read'),
      findsOneWidget,
    );
    expect(find.text('2 MB · nothing in it is unique'), findsOneWidget);

    await tester.tap(
      find.descendant(
        of: find.byKey(const ValueKey('leftover-bot_old')),
        matching: find.text('Discard'),
      ),
    );
    await tester.pumpAndSettle();
    expect(
      calls,
      hasLength(1),
      reason: 'nothing runs before the owner confirms',
    );
    await tester.tap(find.text('Save what differs and discard'));
    await tester.pumpAndSettle();

    final discard = calls[1];
    expect(discard.sublist(1, 4), ['discard', '--agent', 'bot_old']);
    expect(discard, containsAll(['--keep', 'bot_live']));
    expect(
      find.text(
        'Discarded bot_old, freeing 3.2 GB. Saved 2 files and its unlanded '
        'commits to /home/me/.local/share/orgbot/leftovers/bot_old-1.',
      ),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('leftover-bot_old')), findsNothing);
  });

  testWidgets('cancelling the confirmation discards nothing', (tester) async {
    final calls = <List<String>>[];
    await tester.pumpWidget(
      _host(
        runner: (executable, arguments) async {
          calls.add(arguments);
          return _result({
            'ok': true,
            'removed': [],
            'kept': [
              {
                'agent': 'bot_old',
                'reason': '3 change(s) never merged into the project',
                'bytes': 10,
              },
            ],
          });
        },
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Discard'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(calls, hasLength(1));
  });

  testWidgets('a helper error is shown instead of the list', (tester) async {
    await tester.pumpWidget(
      _host(
        runner: (_, _) async => ProcessResult(
          1,
          1,
          jsonEncode({'ok': false, 'error': 'Room state is not private'}),
          '',
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    expect(find.text('Room state is not private'), findsOneWidget);
    expect(find.text('No leftover copies.'), findsNothing);
  });

  test('a discard that found nothing different says so', () {
    expect(
      discardNotice({
        'agent': 'bot_x',
        'rescued': 0,
        'history': false,
        'freed_bytes': 41,
      }),
      'Discarded bot_x, freeing 41 bytes. Nothing in it differed from the project.',
    );
  });
}

Widget _host({required WorkspaceProcessRunner runner}) => MaterialApp(
  home: Scaffold(
    body: Builder(
      builder: (context) => TextButton(
        onPressed: () => showDialog<void>(
          context: context,
          builder: (_) => LeftoverCopiesDialog(
            stateDirectory: Directory('/tmp/room/workspaces'),
            helperPath: '/test/orgbot-workspace.py',
            keep: const ['bot_live', 'bot_live-verify'],
            processRunner: runner,
          ),
        ),
        child: const Text('Open'),
      ),
    ),
  ),
);

ProcessResult _result(Map<String, Object?> value) =>
    ProcessResult(0, 0, jsonEncode(value), '');
