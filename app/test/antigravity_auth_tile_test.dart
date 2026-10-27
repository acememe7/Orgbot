import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/ui/antigravity_auth_tile.dart';
import 'package:orgbot_app/ui/workspace_helper.dart';

void main() {
  test('helper lookup is anchored to known executable layouts', () async {
    final sourceRoot = await Directory.systemTemp.createTemp('agy-source-');
    addTearDown(() => sourceRoot.delete(recursive: true));
    final sourceHelper = File(
      '${sourceRoot.path}/app/packaging/orgbot-workspace.py',
    );
    await sourceHelper.parent.create(recursive: true);
    await sourceHelper.writeAsString('source helper');

    final fromSource = await locateWorkspaceHelper(
      executablePath: '${sourceRoot.path}/app/build/linux/orgbot_app',
    );
    expect(fromSource, sourceHelper.path);

    final fromInstalled = await locateWorkspaceHelper(
      executablePath: '/usr/bin/orgbot',
    );
    expect(
      fromInstalled,
      isNot('${Directory.current.path}/app/packaging/orgbot-workspace.py'),
    );
  });

  testWidgets(
    'status is requested only after an owner action and stays pending',
    (tester) async {
      final calls = <(String, List<String>)>[];
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: AntigravityAuthTile(
              helperPath: '/usr/lib/orgbot/orgbot-workspace.py',
              processRunner: (executable, arguments) async {
                calls.add((executable, arguments));
                return _result({'ok': true, 'credentials_present': true});
              },
            ),
          ),
        ),
      );

      expect(calls, isEmpty);
      expect(find.text('Sign-in status has not been checked.'), findsOneWidget);
      expect(find.text('Uses a separate OrgBots sign-in.'), findsOneWidget);
      await tester.tap(find.text('Check sign-in'));
      await tester.pumpAndSettle();

      expect(calls, hasLength(1));
      expect(calls.single.$1, '/usr/bin/python3');
      expect(calls.single.$2, [
        '/usr/lib/orgbot/orgbot-workspace.py',
        'agy-auth',
        'status',
      ]);
      expect(
        find.text('Private sign-in stored · account checked when bot runs.'),
        findsOneWidget,
      );
      expect(find.text('Signed in'), findsNothing);
      expect(find.text('API key'), findsNothing);
    },
  );

  testWidgets('sign in opens helper in a dedicated argv-safe terminal', (
    tester,
  ) async {
    final launches = <(String, List<String>)>[];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AntigravityAuthTile(
            helperPath: '/usr/lib/orgbot/orgbot-workspace.py',
            terminalLauncher: (executable, arguments) async {
              launches.add((executable, arguments));
            },
          ),
        ),
      ),
    );

    await tester.tap(find.text('Sign in'));
    await tester.pumpAndSettle();

    expect(launches, hasLength(1));
    expect(launches.single.$1, 'x-terminal-emulator');
    expect(launches.single.$2, [
      '-e',
      '/usr/bin/python3',
      '/usr/lib/orgbot/orgbot-workspace.py',
      'agy-auth',
      'login',
    ]);
    expect(
      find.text('Complete sign-in in the new terminal, then check status.'),
      findsOneWidget,
    );
  });

  testWidgets(
    'terminal launch failure exposes the exact quoted helper command',
    (tester) async {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: AntigravityAuthTile(
              helperPath: "/tmp/OrgBot helper's.py",
              terminalLauncher: (_, _) async {
                throw ProcessException('x-terminal-emulator', [], 'not found');
              },
            ),
          ),
        ),
      );

      await tester.tap(find.text('Sign in'));
      await tester.pumpAndSettle();

      expect(
        find.text(
          "'/usr/bin/python3' '/tmp/OrgBot helper'\\''s.py' agy-auth login",
        ),
        findsOneWidget,
      );
      expect(find.text('orgbot agy login'), findsNothing);
    },
  );

  testWidgets('missing sign-in metadata is distinct from ready status', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AntigravityAuthTile(
            helperPath: '/usr/lib/orgbot/orgbot-workspace.py',
            processRunner: (_, _) async =>
                _result({'ok': true, 'credentials_present': false}),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Check sign-in'));
    await tester.pumpAndSettle();

    expect(find.text('No private sign-in is stored yet.'), findsOneWidget);
    expect(find.text('Ready'), findsNothing);
    expect(find.text('Authenticated'), findsNothing);
  });

  testWidgets('malformed helper response fails closed', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AntigravityAuthTile(
            helperPath: '/usr/lib/orgbot/orgbot-workspace.py',
            processRunner: (_, _) async => ProcessResult(1, 0, '{', ''),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Check sign-in'));
    await tester.pumpAndSettle();

    expect(find.text('Sign-in status has not been checked.'), findsNothing);
    expect(
      find.text('The sign-in status response was invalid.'),
      findsOneWidget,
    );
    expect(find.text('Ready'), findsNothing);
  });
}

ProcessResult _result(Map<String, Object?> value) =>
    ProcessResult(1, 0, jsonEncode(value), '');
