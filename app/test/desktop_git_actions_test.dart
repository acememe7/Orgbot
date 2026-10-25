import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/ui/desktop_project_pane.dart';
import 'package:orgbot_app/workspace/desktop_files.dart';

Future<ProcessResult> _git(String root, List<String> arguments) async {
  final result = await Process.run(
    'git',
    arguments,
    workingDirectory: root,
    runInShell: false,
  );
  if (result.exitCode != 0) throw StateError('${result.stderr}');
  return result;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('Git branch input rejects option and revision syntax', () {
    expect(DesktopProjectPane.isValidGitBranch('feature/workspace'), isTrue);
    expect(DesktopProjectPane.isValidGitBranch('release-1.5'), isTrue);
    expect(DesktopProjectPane.isValidGitBranch('--upload-pack=evil'), isFalse);
    expect(DesktopProjectPane.isValidGitBranch('feature/../outside'), isFalse);
    expect(DesktopProjectPane.isValidGitBranch('feature name'), isFalse);
    expect(DesktopProjectPane.isValidGitBranch('feature/@{1}'), isFalse);
  });

  test('bounded Git runner applies local workspace operations', () async {
    final root = await Directory.systemTemp.createTemp('orgbot-git-actions-');
    try {
      await _git(root.path, ['init', '-q']);
      await _git(root.path, ['config', 'user.name', 'OrgBots Test']);
      await _git(root.path, ['config', 'user.email', 'test@example.invalid']);
      await File('${root.path}/README.md').writeAsString('base\n');
      await _git(root.path, ['add', 'README.md']);
      await _git(root.path, ['commit', '-m', 'baseline']);
      final baseBranch = (await _git(root.path, [
        'branch',
        '--show-current',
      ])).stdout.toString().trim();
      await _git(root.path, ['switch', '-c', 'feature/merge-test']);
      await File('${root.path}/feature.txt').writeAsString('feature\n');
      await _git(root.path, ['add', 'feature.txt']);
      await _git(root.path, ['commit', '-m', 'feature file']);
      await _git(root.path, ['switch', baseBranch]);

      await DesktopFiles.boundedGitOutput(root.path, [
        'switch',
        '-c',
        'feature/ui-created',
      ]);
      var current = await _git(root.path, ['branch', '--show-current']);
      expect(current.stdout.toString().trim(), 'feature/ui-created');

      await File('${root.path}/staged.txt').writeAsString('staged\n');
      await _git(root.path, ['add', 'staged.txt']);
      await DesktopFiles.boundedGitOutput(root.path, [
        'commit',
        '-m',
        'commit from workspace UI',
      ]);
      current = await _git(root.path, ['log', '-1', '--format=%s']);
      expect(current.stdout.toString().trim(), 'commit from workspace UI');

      await DesktopFiles.boundedGitOutput(root.path, [
        'merge',
        '--no-edit',
        '--no-ff',
        'feature/merge-test',
      ]);
      current = await _git(root.path, ['show', 'HEAD:feature.txt']);
      expect(current.stdout.toString(), 'feature\n');
    } finally {
      await root.delete(recursive: true);
    }
  });

  testWidgets('Git dialogs pass only validated arguments to the runner', (
    tester,
  ) async {
    late Directory root;
    final actions = <List<String>>[];
    await tester.runAsync(() async {
      root = await Directory.systemTemp.createTemp('orgbot-git-dialog-');
    });
    addTearDown(() async => root.delete(recursive: true));
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SizedBox(
            width: 360,
            height: 620,
            child: DesktopProjectPane(
              projectPath: null,
              initialRootForTesting: root.path,
              gitOutputForTesting: (_, _) async => '## test-branch',
              gitActionForTesting: (_, args) async {
                actions.add(List<String>.of(args));
                return 'Operation completed.';
              },
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Git'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 250));

    await tester.tap(find.text('Branch'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 250));
    await tester.enterText(find.byType(TextField), 'feature/ui-created');
    await tester.tap(find.text('Create'));
    await tester.pump(const Duration(milliseconds: 250));
    await tester.pump();

    await tester.tap(find.text('Commit staged'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 250));
    await tester.enterText(find.byType(TextField), 'workspace commit');
    await tester.tap(find.text('Commit staged').last);
    await tester.pump(const Duration(milliseconds: 250));
    await tester.pump();

    await tester.tap(find.text('Merge'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 250));
    await tester.enterText(find.byType(TextField), 'feature/merge-test');
    await tester.tap(find.text('Merge').last);
    await tester.pump(const Duration(milliseconds: 250));
    await tester.pump();

    expect(actions, [
      ['switch', '-c', 'feature/ui-created'],
      ['commit', '-m', 'workspace commit'],
      ['merge', '--no-edit', '--no-ff', 'feature/merge-test'],
    ]);
  });
}
