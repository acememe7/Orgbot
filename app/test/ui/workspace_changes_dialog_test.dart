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
          'These exact project files will be moved to your configured recovery folder before the changes are applied:',
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
}

Widget _host({required WorkspaceProcessRunner runner}) => MaterialApp(
  home: Scaffold(
    body: Builder(
      builder: (context) => TextButton(
        onPressed: () => showDialog<void>(
          context: context,
          builder: (_) => WorkspaceChangesDialog(
            stateDirectory: Directory('/tmp/room/workspaces'),
            helperPath: '/test/orgbot-workspace.py',
            agents: const [
              WorkspaceAgentLabel(id: 'agent-uuid', label: '@astra · engine'),
            ],
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
