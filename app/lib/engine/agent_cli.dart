// Bots backed by the agent CLIs already signed in on this machine.
//
// This is the answer to "I have subscriptions, stop asking me for API keys".
// A bot backed by `claude`, `codex` or `grok` spawns that CLI, which brings its
// own sign-in — so a Claude Pro/Max, ChatGPT/Codex or Grok subscription is used
// directly and no key is ever entered or stored. Only a raw API endpoint such
// as DeepSeek still needs one.
//
// The invocations here mirror orgbot-providers::agent_cli in the Rust core,
// which is the proven version; the difference is what comes back. The Rust
// build hands the CLI an MCP relay so it can drive the board with real tool
// calls. There is no such relay on-device, so instead the room's tool surface
// is described in the prompt and the reply is parsed for a JSON action. That
// keeps a CLI bot a full participant — it can claim, work and submit — rather
// than a commentator that can only talk.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:convert';
import 'dart:io';

import 'engine_core.dart';
import 'llm_provider.dart';
import 'runner.dart';

/// Which agent CLI backs a bot.
enum CliKind {
  claude('claude', 'Anthropic', 'Claude Pro/Max sign-in'),
  codex('codex', 'Codex / ChatGPT', 'ChatGPT sign-in'),
  grok('grok', 'Grok', 'grok.com sign-in');

  /// The binary name on PATH.
  final String binary;

  /// What the picker shows.
  final String label;

  /// How this CLI authenticates, shown instead of an API key field.
  final String signInNote;

  const CliKind(this.binary, this.label, this.signInNote);

  static CliKind? fromBinary(String name) {
    for (final k in values) {
      if (k.binary == name) return k;
    }
    return null;
  }
}

/// Where a CLI binary was found, or why it was not.
class CliStatus {
  /// The resolved absolute path, or null when the CLI is unavailable.
  final String? path;

  /// A short explanation when [path] is null.
  final String? problem;

  const CliStatus(this.path, this.problem);

  bool get available => path != null;
}

/// Find a CLI binary on PATH and in the usual per-user install locations.
///
/// `which` is not used: these CLIs are commonly installed under ~/.local/bin
/// or ~/.npm-global/bin or ~/.grok/bin, which a non-login shell's PATH may not
/// carry. Keep this order aligned with the Rust core's `locate` implementation.
CliStatus findCli(CliKind cli, {String? path, String? home}) {
  if (!canRunAgentClis) {
    return const CliStatus(null,
        'Agent CLIs run on desktop only — this platform cannot start one.');
  }
  final searchHome = home ?? Platform.environment['HOME'] ?? '';
  final candidates = <String>[
    for (final dir in (path ?? Platform.environment['PATH'] ?? '').split(':'))
      if (dir.isNotEmpty) '$dir/${cli.binary}',
    if (searchHome.isNotEmpty) ...[
      '$searchHome/.local/bin/${cli.binary}',
      '$searchHome/.npm-global/bin/${cli.binary}',
      '$searchHome/.grok/bin/${cli.binary}',
    ],
  ];
  for (final candidate in candidates) {
    if (_isExecutable(candidate)) return CliStatus(candidate, null);
  }
  return CliStatus(
      null, '${cli.binary} is not installed, or not on this app\'s PATH.');
}

bool _isExecutable(String path) {
  try {
    final file = File(path);
    if (!file.existsSync()) return false;
    final stat = file.statSync();
    if (stat.type != FileSystemEntityType.file) return false;
    // A readable regular file that the OS will exec. `stat` is cheap and
    // avoids spawning anything just to test presence.
    return stat.mode & 0x49 != 0; // any execute bit
  } catch (_) {
    return false;
  }
}

/// Whether this device can run the agent CLIs at all.
///
/// A phone cannot, so anything that depends on a CLI must either use a
/// signed-in subscription over the API or be left out of the picker.
bool get canRunAgentClis =>
    Platform.isLinux || Platform.isMacOS || Platform.isWindows;

/// A bot that runs one of the machine's agent CLIs as a resumable session.
class AgentCliBot implements BotProvider {
  final CliKind cli;
  final String binPath;
  final String handle;
  final String system;

  /// The model to pass with `-m`. Empty means the CLI's own default, which
  /// follows whatever the subscription currently provides.
  final String model;

  /// Where the CLI runs. Empty means this process's directory.
  final String workdir;

  /// Seconds before a turn is abandoned. A CLI agent can think for a long
  /// time; without a ceiling one wedged turn stalls the whole room.
  final int timeoutSeconds;

  /// Set on the first turn, resumed on every turn after, so the bot keeps its
  /// working memory across the room run.
  String? _session;

  /// Overridable for tests: runs the CLI and returns its stdout.
  final Future<CliResult> Function(List<String> args, String? stdinText)? _run;

  AgentCliBot({
    required this.cli,
    required this.binPath,
    required this.handle,
    required this.system,
    this.model = '',
    this.workdir = '',
    this.timeoutSeconds = 300,
    Future<CliResult> Function(List<String> args, String? stdinText)? runner,
  }) : _run = runner;

  /// Build a bot, or throw with a readable reason when the CLI is missing.
  factory AgentCliBot.resolve({
    required CliKind cli,
    required String handle,
    required String system,
    String model = '',
    String workdir = '',
  }) {
    final status = findCli(cli);
    if (!status.available) {
      throw EngineError(status.problem ?? 'cannot run ${cli.binary}');
    }
    return AgentCliBot(
      cli: cli,
      binPath: status.path!,
      handle: handle,
      system: system,
      model: model,
      workdir: workdir,
    );
  }

  @override
  String get name => cli.label;

  /// The session being resumed, once one exists.
  String? get sessionId => _session;

  @override
  Future<BotTurn> act({required String situation}) async {
    // A fresh session needs the standing brief and the protocol; a resumed one
    // already has both and only needs what changed, which keeps each turn's
    // prompt small.
    final prompt = _session == null
        ? '$system\n\n${_protocol()}\n\n$situation'
        : '$situation\n\n${_replyContract()}';

    final args = buildArgs(
      cli: cli,
      prompt: prompt,
      resumed: _session,
      model: model,
    );

    final result = await (_run ?? _spawn)(args, null);
    final episode = parseEpisode(cli, result.stdout);
    if (episode.sessionId != null) _session = episode.sessionId;

    if (episode.text.trim().isEmpty) {
      // Nothing usable came back. Surface the CLI's own complaint rather than
      // a silent empty turn, which would read as the bot having nothing to do.
      final complaint = result.stderr.trim();
      if (result.exitCode != 0 && complaint.isNotEmpty) {
        throw EngineError(
            '${cli.binary} exited ${result.exitCode}: ${_firstLines(complaint, 3)}');
      }
      return const BotTurn();
    }
    return parseAgentReply(episode.text);
  }

  Future<CliResult> _spawn(List<String> args, String? stdinText) async {
    final process = await Process.start(
      binPath,
      args,
      workingDirectory: workdir.isEmpty ? null : workdir,
      environment: const {'NO_COLOR': '1'},
    );
    await process.stdin.close();

    final out = StringBuffer();
    final err = StringBuffer();
    final collecting = Future.wait([
      process.stdout.transform(utf8.decoder).forEach(out.write),
      process.stderr.transform(utf8.decoder).forEach(err.write),
    ]);

    int code;
    try {
      code = await process.exitCode.timeout(Duration(seconds: timeoutSeconds));
      await collecting.timeout(const Duration(seconds: 5), onTimeout: () => []);
    } catch (_) {
      process.kill(ProcessSignal.sigkill);
      throw EngineError(
          '${cli.binary} did not finish within ${timeoutSeconds}s; the turn was abandoned.');
    }
    return CliResult(code, out.toString(), err.toString());
  }

  /// The standing explanation of how to act on the board, sent once per session.
  String _protocol() {
    final buffer = StringBuffer()
      ..writeln('You are @$handle, one bot in a room of bots sharing a task board.')
      ..writeln('You act by replying with a single JSON object and nothing else.')
      ..writeln('')
      ..writeln('Shape:')
      ..writeln('  {"say": "<optional message to the room>",')
      ..writeln('   "actions": [{"tool": "<name>", "args": {...}}]}')
      ..writeln('')
      ..writeln('Tools you may call:');
    for (final tool in engineTools()) {
      final props = (tool.inputSchema['properties'] as Map?)?.keys.join(', ') ?? '';
      buffer.writeln('  ${tool.name}($props) — ${tool.description}');
    }
    buffer
      ..writeln('')
      ..writeln('Rules: claim a task before working it; record your work with '
          'add_artifact then submit_for_review; you may not inspect your own '
          'work. Return {"actions": []} when there is nothing useful to do.')
      ..writeln(_replyContract());
    return buffer.toString();
  }

  String _replyContract() =>
      'Reply with the JSON object only — no prose around it, no code fence.';

  static String _firstLines(String text, int count) =>
      text.split('\n').take(count).join(' ').trim();
}

/// What one CLI run produced. Public because it appears in the
/// injectable runner signature that tests substitute.
class CliResult {
  final int exitCode;
  final String stdout;
  final String stderr;
  const CliResult(this.exitCode, this.stdout, this.stderr);
}

/// One episode's accumulated output.
class Episode {
  final StringBuffer _text = StringBuffer();
  String? sessionId;

  String get text => _text.toString();
  void write(String s) => _text.write(s);
}

/// Build the argv for a CLI, mirroring the Rust core's proven invocations.
///
/// Options must precede the trailing prompt for codex resume, and the model
/// flag goes last so it applies to every form.
List<String> buildArgs({
  required CliKind cli,
  required String prompt,
  String? resumed,
  String model = '',
}) {
  final args = <String>[];
  switch (cli) {
    case CliKind.codex:
      if (resumed == null) {
        args.addAll(['exec', '--json', '--skip-git-repo-check', prompt]);
      } else {
        // A resumed session keeps the sandbox it started with, so no -s here.
        args.addAll(
            ['exec', 'resume', '--json', '--skip-git-repo-check', resumed, prompt]);
      }
    case CliKind.claude:
      args.addAll(['-p', prompt, '--output-format', 'stream-json', '--verbose']);
      if (resumed != null) args.addAll(['--resume', resumed]);
    case CliKind.grok:
      args.addAll(['-p', prompt, '--output-format', 'streaming-messages-json']);
      if (resumed != null) args.addAll(['-r', resumed]);
  }
  if (model.isNotEmpty) args.addAll(['-m', model]);
  return args;
}

/// Read a CLI's NDJSON stream into an [Episode].
///
/// Codex speaks its own event vocabulary; Claude and Grok both emit the
/// Anthropic wire shape as NDJSON. Lines that are not JSON are banners or
/// warnings and are skipped rather than failing the turn.
Episode parseEpisode(CliKind cli, String stdout) {
  final episode = Episode();
  for (final line in const LineSplitter().convert(stdout)) {
    final trimmed = line.trim();
    if (trimmed.isEmpty) continue;
    Object? decoded;
    try {
      decoded = jsonDecode(trimmed);
    } catch (_) {
      continue;
    }
    if (decoded is! Map<String, Object?>) continue;
    if (cli == CliKind.codex) {
      _absorbCodex(decoded, episode);
    } else {
      _absorbAnthropicStyle(decoded, episode);
    }
  }
  return episode;
}

void _absorbCodex(Map<String, Object?> event, Episode episode) {
  final type = event['type'] as String?;
  if (type == 'thread.started') {
    final id = event['thread_id'] ?? event['session_id'] ?? event['id'];
    if (id is String) episode.sessionId = id;
    return;
  }
  if (type == 'item.completed') {
    final item = event['item'];
    if (item is Map && item['type'] == 'agent_message') {
      final text = item['text'];
      if (text is String) episode.write(text);
    }
  }
}

void _absorbAnthropicStyle(Map<String, Object?> event, Episode episode) {
  final id = event['session_id'];
  if (id is String && id.isNotEmpty) episode.sessionId = id;

  // The terminal result line carries the whole answer on both CLIs.
  if (event['type'] == 'result') {
    final result = event['result'];
    if (result is String) episode.write(result);
    return;
  }
  // Assistant messages arrive as Anthropic content blocks.
  if (event['type'] == 'assistant') {
    final message = event['message'];
    if (message is Map) {
      final content = message['content'];
      if (content is List) {
        for (final block in content) {
          if (block is Map && block['type'] == 'text' && block['text'] is String) {
            episode.write(block['text'] as String);
          }
        }
      }
    }
  }
}

/// Turn an agent's reply into a room turn.
///
/// The agent is asked for a bare JSON object, but CLIs wrap answers in prose
/// or a code fence often enough that being strict here would waste turns. The
/// outermost balanced object is extracted; if there is none, the whole reply
/// becomes something the bot says, which is still a useful contribution.
BotTurn parseAgentReply(String reply) {
  final json = _extractObject(reply);
  if (json == null) {
    final text = reply.trim();
    return BotTurn(text: text.isEmpty ? null : text);
  }
  Object? decoded;
  try {
    decoded = jsonDecode(json);
  } catch (_) {
    final text = reply.trim();
    return BotTurn(text: text.isEmpty ? null : text);
  }
  if (decoded is! Map<String, Object?>) {
    return BotTurn(text: reply.trim());
  }

  final calls = <ToolCall>[];
  final actions = decoded['actions'];
  if (actions is List) {
    for (final action in actions) {
      if (action is! Map) continue;
      final name = action['tool'] ?? action['name'];
      if (name is! String || name.isEmpty) continue;
      final rawArgs = action['args'] ?? action['arguments'] ?? const {};
      calls.add(ToolCall(
        name,
        rawArgs is Map ? rawArgs.cast<String, Object?>() : const {},
      ));
    }
  }
  final said = decoded['say'] ?? decoded['text'] ?? decoded['message'];
  final text = said is String && said.trim().isNotEmpty ? said.trim() : null;
  return BotTurn(tools: calls, text: text);
}

/// Extract the outermost balanced `{...}` from text, ignoring braces inside
/// strings so a message containing one does not truncate the object.
String? _extractObject(String text) {
  final start = text.indexOf('{');
  if (start < 0) return null;
  var depth = 0;
  var inString = false;
  var escaped = false;
  for (var i = start; i < text.length; i++) {
    final c = text[i];
    if (inString) {
      if (escaped) {
        escaped = false;
      } else if (c == r'\') {
        escaped = true;
      } else if (c == '"') {
        inString = false;
      }
      continue;
    }
    if (c == '"') {
      inString = true;
    } else if (c == '{') {
      depth++;
    } else if (c == '}') {
      depth--;
      if (depth == 0) return text.substring(start, i + 1);
    }
  }
  return null;
}
