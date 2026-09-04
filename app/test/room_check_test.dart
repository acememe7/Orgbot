// Whether a roster can actually carry work end to end.
//
// The case that prompted this: one engineer and one task. It looks fine, runs,
// and then parks forever at "0 of 1 delivered, 1 in flight" — because a bot
// may not inspect its own work and there is nobody else.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/orgbot/room_check.dart';
import 'package:orgbot_app/orgbot/roles.dart';

BotDef bot(String handle, String role) =>
    BotDef(name: handle, handle: handle, role: role);

RoomDef room({List<String> tasks = const ['do the thing'], String goal = 'ship'}) =>
    RoomDef(name: 'r', goal: goal, tasks: tasks);

List<String> summaries(List<RoomIssue> issues) =>
    issues.map((i) => i.summary).toList();

void main() {
  test('the exact room that stalled is reported as unable to finish', () {
    final issues = checkRoom(room(), [bot('dennis', 'engineer')]);
    expect(hasBlockingIssue(issues), isTrue);
    expect(
      summaries(issues).join(' '),
      allOf(contains('inspect'), contains('deliver')),
      reason: 'a lone engineer can neither review its work nor hand it over',
    );
  });

  test('a full pipeline passes', () {
    final issues = checkRoom(room(), [
      bot('lead', 'coordinator'),
      bot('dev', 'engineer'),
      bot('qa', 'reviewer'),
    ]);
    expect(hasBlockingIssue(issues), isFalse, reason: summaries(issues).join('; '));
  });

  test('a room with no inspector is blocked', () {
    final issues = checkRoom(room(), [
      bot('lead', 'coordinator'),
      bot('dev', 'engineer'),
    ]);
    expect(summaries(issues).join(' '), contains('inspect'));
  });

  test('a room with nobody to deliver is blocked', () {
    final issues = checkRoom(room(), [
      bot('dev', 'engineer'),
      bot('qa', 'reviewer'),
    ]);
    expect(summaries(issues).join(' '), contains('deliver'));
  });

  test('a room of only inspectors has nobody to do the work', () {
    final issues = checkRoom(room(), [
      bot('qa', 'reviewer'),
      bot('sec', 'security'),
      bot('lead', 'coordinator'),
    ]);
    expect(summaries(issues).join(' '), contains('nobody does the work'));
  });

  test('an empty room is reported rather than started', () {
    final issues = checkRoom(room(), []);
    expect(hasBlockingIssue(issues), isTrue);
    expect(summaries(issues).single, contains('no bots'));
  });

  test('one bot can never finish, whatever its permissions', () {
    final solo = bot('solo', 'engineer')
      ..powers = const RolePowers(
          mayCreateTasks: true, mayInspect: true, maySubmit: true);
    final issues = checkRoom(room(), [solo]);
    expect(
      summaries(issues).join(' '),
      contains('one bot'),
      reason: 'inspection must be done by someone other than the author',
    );
  });

  test('no work and nobody who can create any is blocking', () {
    final issues = checkRoom(room(tasks: const []), [
      bot('dev', 'engineer'),
      bot('qa', 'reviewer'),
      bot('dev2', 'writer'),
    ]);
    expect(summaries(issues).join(' '), contains('no work'));
  });

  test('a planner covers an empty board', () {
    final issues = checkRoom(room(tasks: const []), [
      bot('lead', 'coordinator'),
      bot('dev', 'engineer'),
      bot('qa', 'reviewer'),
    ]);
    expect(hasBlockingIssue(issues), isFalse, reason: summaries(issues).join('; '));
  });

  test('a missing goal is advice, not a blocker', () {
    final issues = checkRoom(room(goal: '  '), [
      bot('lead', 'coordinator'),
      bot('dev', 'engineer'),
      bot('qa', 'reviewer'),
    ]);
    expect(hasBlockingIssue(issues), isFalse);
    expect(summaries(issues).join(' '), contains('no goal'));
  });

  test('a lone inspector in a big room is flagged as a bottleneck', () {
    final issues = checkRoom(room(), [
      bot('lead', 'coordinator'),
      bot('qa', 'reviewer'),
      for (var i = 0; i < 4; i++) bot('dev$i', 'engineer'),
    ]);
    expect(hasBlockingIssue(issues), isFalse);
    expect(summaries(issues).join(' '), contains('One inspector'));
  });

  test('every issue says what to do about it', () {
    final issues = checkRoom(room(), [bot('dennis', 'engineer')]);
    for (final issue in issues) {
      expect(issue.fix.trim(), isNotEmpty,
          reason: 'a warning without a fix just makes someone feel stuck');
    }
  });
}
