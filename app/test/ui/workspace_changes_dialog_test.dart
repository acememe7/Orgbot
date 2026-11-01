import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/ui/workspace_changes_dialog.dart';

void main() {
  testWidgets('opening and refreshing only request read-only status', (
    tester,
  ) async {
    final calls = <List<String>>[];
    await tester.pumpWidget(
      _host(
        runner: (executable, arguments) async {
          calls.add(arguments);
          return _result({
            'ok': true,
            'changed': [],
            'added': [],
            'deleted': [],
            'conflicts': [],
          });
        },
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();

    expect(calls, hasLength(1));
    expect(calls.single[1], 'status');
    expect(
      calls.single,
      containsAll(['--state', '/tmp/room/workspaces', '--agent', 'agent-uuid']),
    );
    expect(find.text('No unreviewed workspace changes.'), findsOneWidget);
    expect(
      tester
          .widget<FilledButton>(
            find.widgetWithText(FilledButton, 'Apply changes'),
          )
          .onPressed,
      isNull,
    );
  });

  testWidgets('conflicts prevent apply', (tester) async {
    final calls = <List<String>>[];
    await tester.pumpWidget(
      _host(
        runner: (executable, arguments) async {
          calls.add(arguments);
          return _result({
            'ok': true,
            'changed': [
              {'path': 'src/main.dart', 'kind': 'modified'},
            ],
            'added': [],
            'deleted': [],
            'conflicts': ['src/main.dart'],
          });
        },
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();

    expect(
      find.text(
        'Resolve conflicts in the project before applying any changes.',
      ),
      findsOneWidget,
    );
    expect(
      tester
          .widget<FilledButton>(
            find.widgetWithText(FilledButton, 'Apply changes'),
          )
          .onPressed,
      isNull,
    );
    expect(calls, hasLength(1));
  });

  testWidgets(
    'deletion requires path-specific approval before snapshot-bound apply',
    (tester) async {
      final calls = <List<String>>[];
      var merged = false;
      await tester.pumpWidget(
        _host(
          runner: (executable, arguments) async {
            calls.add(arguments);
            if (arguments[1] == 'apply') {
              merged = true;
              return _result({
                'ok': true,
                'applied': ['obsolete.txt'],
              });
            }
            if (merged) {
              return _result({
                'ok': true,
                'changed': [],
                'added': [],
                'deleted': [],
                'conflicts': [],
              });
            }
            return _result({
              'ok': true,
              'changed': [],
              'added': [],
              'deleted': ['obsolete.txt'],
              'review_token': 'a' * 64,
              'conflicts': [],
            });
          },
        ),
      );
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();

      expect(find.text('obsolete.txt'), findsOneWidget);
      expect(calls, hasLength(1));
      await tester.tap(find.text('Apply changes'));
      await tester.pumpAndSettle();

      expect(
        find.text(
          'This project file will be moved to your configured recovery folder before the changes are applied:',
        ),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('approved-delete-obsolete.txt')),
        findsOneWidget,
      );
      expect(calls, hasLength(1));

      await tester.tap(find.text('Move listed files to Trash and apply'));
      await tester.pumpAndSettle();

      final applyCall = calls.singleWhere(
        (arguments) => arguments[1] == 'apply',
      );
      expect(applyCall, containsAllInOrder(['--review-token', 'a' * 64]));
      expect(applyCall, isNot(contains('--allow-delete')));
      expect(
        calls.where((arguments) => arguments[1] == 'status'),
        hasLength(3),
      );
    },
  );

  testWidgets(
    'a review of 1,570 removals stays responsive and says where they are',
    (tester) async {
      // From the live room: 1,570 removals built as one column of selectable
      // text froze the app when the owner pressed apply.
      final removals = [
        for (var i = 0; i < 759; i++) '.failwith/src/file$i.ts',
        for (var i = 0; i < 759; i++) '.ng/src/file$i.ts',
        for (var i = 0; i < 42; i++) '.nongas/file$i.ts',
        for (var i = 0; i < 10; i++) 'lint$i.out',
      ];
      final calls = <List<String>>[];
      await tester.pumpWidget(
        _host(
          runner: (executable, arguments) async {
            calls.add(arguments);
            return _result({
              'ok': true,
              'changed': [],
              'added': [],
              'deleted': removals,
              'review_token': 'b' * 64,
              'conflicts': [],
            });
          },
        ),
      );
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(find.text('Deleted (1570)'), findsOneWidget);
      expect(find.text('759 files in .failwith/'), findsOneWidget);
      expect(
        find
            .byWidgetPredicate(
              (widget) =>
                  widget.key is ValueKey<String> &&
                  (widget.key! as ValueKey<String>).value.startsWith(
                    'workspace-path-',
                  ),
              skipOffstage: false,
            )
            .evaluate()
            .length,
        lessThan(200),
        reason: 'only the rows on screen are built',
      );

      final started = DateTime.now();
      await tester.tap(find.text('Apply changes'));
      await tester.pumpAndSettle();
      expect(find.textContaining('These 1570 project files'), findsOneWidget);
      expect(find.text('759 files in .ng/'), findsWidgets);
      await tester.tap(find.text('Move listed files to Trash and apply'));
      await tester.pumpAndSettle();
      expect(
        DateTime.now().difference(started),
        lessThan(const Duration(seconds: 5)),
      );
      expect(calls.where((arguments) => arguments[1] == 'apply'), hasLength(1));
    },
  );

  test(
    'the helper\'s progress lines read as counts, and other lines are left alone',
    () {
      expect(
        workspaceProgressText(
          '{"progress": {"stage": "trash", "done": 820, "total": 1549}}',
        ),
        'Moved 820 of 1549 to the trash',
      );
      expect(
        workspaceProgressText(
          '{"progress": {"stage": "apply", "done": 3, "total": 9}}',
        ),
        'Applied 3 of 9 changes',
      );
      expect(
        workspaceProgressText('orgbot-trash refused to preserve x'),
        isNull,
      );
      expect(workspaceProgressText('{"progress": "broken"'), isNull);
    },
  );

  testWidgets('helper failures are visible and do not enable apply', (
    tester,
  ) async {
    await tester.pumpWidget(
      _host(
        runner: (executable, arguments) async => ProcessResult(
          0,
          2,
          jsonEncode({'ok': false, 'error': 'No workspace manifest.'}),
          '',
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();

    expect(find.text('No workspace manifest.'), findsOneWidget);
    expect(
      tester
          .widget<FilledButton>(
            find.widgetWithText(FilledButton, 'Apply changes'),
          )
          .onPressed,
      isNull,
    );
  });
  testWidgets(
    'opens on the copy with the most removals and corrects stale counts',
    (tester) async {
      // From the live room: the badge said 2,177 and nobody could tell which
      // bot owned them; 1,588 were in a copy that had none left.
      final asked = <String>[];
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Builder(
              builder: (context) => TextButton(
                onPressed: () => showDialog<void>(
                  context: context,
                  builder: (_) => WorkspaceChangesDialog(
                    stateDirectory: Directory('/tmp/room/workspaces'),
                    helperPath: '/test/orgbot-workspace.py',
                    agents: const [
                      WorkspaceAgentLabel(
                        id: 'bot_mira',
                        label: '@mira · room tools',
                      ),
                      WorkspaceAgentLabel(
                        id: 'bot_quinn',
                        label: '@ollama-glm · room tools',
                        removals: 1588,
                      ),
                      WorkspaceAgentLabel(
                        id: 'bot_finn',
                        label: '@grok-quick · room tools',
                        removals: 562,
                      ),
                    ],
                    processRunner: (executable, arguments) async {
                      final agent = arguments[arguments.indexOf('--agent') + 1];
                      asked.add(agent);
                      return _result({
                        'ok': true,
                        'changed': [],
                        'added': [],
                        'deleted': agent == 'bot_finn'
                            ? ['.freeze-probe/a', '.freeze-probe/b']
                            : [],
                        'conflicts': [],
                      });
                    },
                  ),
                ),
                child: const Text('Open'),
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(
        asked,
        ['bot_quinn', 'bot_finn'],
        reason:
            'the largest count first, then the other non-zero one; never the copy with none',
      );
      expect(
        find.text('@ollama-glm · room tools'),
        findsOneWidget,
        reason: 'the stale 1,588 is corrected to none',
      );
      await tester.tap(find.byType(DropdownButtonFormField<String>));
      await tester.pumpAndSettle();
      expect(find.text('@grok-quick · room tools · 2 removals'), findsWidgets);
    },
  );

  testWidgets(
    'a removal review says what each folder holds and flags recent changes',
    (tester) async {
      // From the live room: .freeze-probe/ was among 562 removals and had been
      // written to minutes before.
      final now = DateTime(2026, 10, 31, 23, 40);
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: RemovalGroups(
              now: now,
              groups: [
                {
                  'folder': '.freeze-probe/',
                  'files': 560,
                  'bytes': 135000000,
                  'tracked': 0,
                  'newest':
                      now
                          .subtract(const Duration(minutes: 12))
                          .millisecondsSinceEpoch ~/
                      1000,
                },
                {
                  'folder': 'lint-rerun-note.txt',
                  'files': 1,
                  'bytes': 900,
                  'tracked': 1,
                  'newest':
                      now
                          .subtract(const Duration(hours: 3))
                          .millisecondsSinceEpoch ~/
                      1000,
                },
              ],
            ),
          ),
        ),
      );
      expect(
        find.textContaining('560 files · 135 MB · not in git'),
        findsOneWidget,
      );
      expect(find.text('changed 12 min ago, may be in use'), findsOneWidget);
      expect(
        find.textContaining('1 file · 900 bytes · in git'),
        findsOneWidget,
      );
      expect(find.text('changed 3 h ago'), findsOneWidget);
    },
  );

  test('a removal group says which bots added and last changed its files', () {
    final now = DateTime(2026, 11, 1, 2, 0);
    final at = now.subtract(const Duration(hours: 2)).millisecondsSinceEpoch;
    expect(
      removalOrigin({
        'files': 40,
        'made_by': [
          ['@sol', 30],
          ['@mira', 4],
        ],
        'last_by': '@mira',
        'last_at': at ~/ 1000,
      }, now),
      'added by @sol (30), @mira (4) · 6 with no record · last changed by @mira 2 h ago',
    );
    // Older files, from before OrgBots kept a record: git says when they
    // came, though not which bot wrote them.
    final came = DateTime(2026, 10, 29, 18).millisecondsSinceEpoch ~/ 1000;
    expect(
      removalOrigin({
        'files': 560,
        'git_files': 558,
        'git_added': {
          'commit': '9c92486',
          'at': came,
          'subject': 'Curfew squads',
          'commits': 1,
        },
      }, now),
      '558 came in with 9c92486 "Curfew squads" on 29 Oct · 2 with no record',
    );
    expect(
      removalOrigin({
        'files': 4,
        'git_files': 4,
        'git_added': {
          'commit': 'cf4f8f3',
          'at': came,
          'subject': 'Protocol 5',
          'commits': 3,
        },
      }, now),
      'came in over 3 commits, latest cf4f8f3 "Protocol 5" on 29 Oct',
    );
    expect(
      removalOrigin({
        'files': 2,
        'made_by': [
          ['@sol', 2],
        ],
      }, now),
      'added by @sol',
    );
    expect(removalOrigin({'files': 3}, now), isNull);
  });

  testWidgets('the owner approves the removals in some folders only', (
    tester,
  ) async {
    // From the live room: one approval had to take all 562 removals or none.
    final calls = <List<String>>[];
    var merged = false;
    final groups = [
      {'folder': '.freeze-probe/', 'files': 2, 'bytes': 20, 'tracked': 0},
      {'folder': 'logs/', 'files': 1, 'bytes': 10, 'tracked': 0},
    ];
    await tester.pumpWidget(
      _host(
        runner: (executable, arguments) async {
          calls.add(arguments);
          if (arguments[1] == 'apply') {
            merged = true;
            return _result({'ok': true, 'applied': []});
          }
          return _result({
            'ok': true,
            'changed': [],
            'added': [],
            'deleted': merged
                ? ['.freeze-probe/a.js', '.freeze-probe/b.js']
                : ['.freeze-probe/a.js', '.freeze-probe/b.js', 'logs/old.txt'],
            'deleted_groups': merged ? [groups.first] : groups,
            'review_token': 'a' * 64,
            'conflicts': [],
          });
        },
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Apply changes'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('choose-folders')));
    await tester.pumpAndSettle();
    await tester.tap(
      find.byKey(const ValueKey('choose-folder-.freeze-probe/')),
    );
    await tester.pumpAndSettle();
    expect(find.text('Move 1 file to Trash and apply'), findsOneWidget);
    await tester.tap(find.text('Move 1 file to Trash and apply'));
    await tester.pumpAndSettle();
    final apply = calls.singleWhere((arguments) => arguments[1] == 'apply');
    expect(apply, containsAllInOrder(['--review-token', 'a' * 64]));
    expect(apply, containsAllInOrder(['--only-folder', 'logs/']));
    expect(apply, isNot(contains('.freeze-probe/')));
  });

  testWidgets('the review shows every copy with removals waiting', (
    tester,
  ) async {
    // From the live room: 2,177 removals and the owner could not tell which
    // bots they belonged to without opening the picker.
    final asked = <String>[];
    await tester.pumpWidget(
      _host(
        agents: const [
          WorkspaceAgentLabel(id: 'bot_mira', label: '@mira', removals: 1370),
          WorkspaceAgentLabel(id: 'bot_sol', label: '@sol', removals: 562),
          WorkspaceAgentLabel(id: 'bot_iris', label: '@iris'),
        ],
        runner: (executable, arguments) async {
          final agent = arguments[arguments.indexOf('--agent') + 1];
          asked.add(agent);
          return _result({
            'ok': true,
            'changed': [],
            'added': [],
            'deleted': [
              for (var i = 0; i < (agent == 'bot_mira' ? 1370 : 562); i++)
                'f$i.txt',
            ],
            'review_token': 'a' * 64,
            'conflicts': [],
          });
        },
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    expect(find.text('1932 removals waiting in 2 copies'), findsOneWidget);
    expect(find.byKey(const ValueKey('copy-chip-bot_iris')), findsNothing);
    await tester.tap(find.byKey(const ValueKey('copy-chip-bot_sol')));
    await tester.pumpAndSettle();
    expect(asked.last, 'bot_sol', reason: 'a chip opens that copy');
  });
}

Widget _host({
  required WorkspaceProcessRunner runner,
  List<WorkspaceAgentLabel> agents = const [
    WorkspaceAgentLabel(id: 'agent-uuid', label: '@astra · room tools'),
  ],
}) => MaterialApp(
  home: Scaffold(
    body: Builder(
      builder: (context) => TextButton(
        onPressed: () => showDialog<void>(
          context: context,
          builder: (_) => WorkspaceChangesDialog(
            stateDirectory: Directory('/tmp/room/workspaces'),
            helperPath: '/test/orgbot-workspace.py',
            agents: agents,
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
