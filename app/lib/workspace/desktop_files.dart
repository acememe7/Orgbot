import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/services.dart';

import '../protocol/models.dart';
import '../state/room_archive.dart';

const desktopAttachmentsChannel = MethodChannel('orgbot/attachments');
const desktopWorkspaceChannel = MethodChannel('orgbot/workspace');
const desktopWorkspaceEditorChannel = MethodChannel('orgbot/workspace_editor');

class DesktopFiles {
  static const maxFiles = 5;
  static const maxBytes = 20 * 1024 * 1024;
  static const maxTextBytes = 512 * 1024;
  static const maxPreviewBytes = 2 * 1024 * 1024;
  static const maxEditableBytes = 2 * 1024 * 1024;

  static Future<Directory> roomAttachmentDirectory(String roomId) async =>
      Directory('${(await RoomArchive.directory(roomId)).path}/attachments');

  static Future<String?> chooseProjectFolder() =>
      desktopWorkspaceChannel.invokeMethod<String>('choose_directory');

  static Future<Uint8List> readProjectFile(String root, String path) async {
    final canonicalRoot = await Directory(root).resolveSymbolicLinks();
    final canonicalFile = await File(path).resolveSymbolicLinks();
    if (!canonicalFile.startsWith('$canonicalRoot${Platform.pathSeparator}')) {
      throw const FileSystemException('This path leaves the selected project.');
    }
    final relative = canonicalFile.substring(canonicalRoot.length + 1);
    final bytes = await desktopWorkspaceChannel.invokeMethod<Uint8List>(
      'read_preview',
      {'root': canonicalRoot, 'path': relative},
    );
    if (bytes == null || bytes.length > maxBytes) {
      throw const FileSystemException('File is too large to preview.');
    }
    return bytes;
  }

  static Future<EditableProjectFile> readEditableProjectFile(
    String root,
    String path,
  ) async {
    final canonicalRoot = await Directory(root).resolveSymbolicLinks();
    final canonicalFile = await File(path).resolveSymbolicLinks();
    if (!canonicalFile.startsWith('$canonicalRoot${Platform.pathSeparator}')) {
      throw const FileSystemException('This path leaves the selected project.');
    }
    final relative = canonicalFile.substring(canonicalRoot.length + 1);
    final result = await desktopWorkspaceEditorChannel
        .invokeMapMethod<String, Object?>('read', {
          'root': canonicalRoot,
          'path': relative,
        });
    final bytes = result?['bytes'];
    final token = result?['token'];
    if (bytes is! Uint8List ||
        token is! String ||
        bytes.length > maxEditableBytes) {
      throw const FormatException('The project editor returned invalid data.');
    }
    return EditableProjectFile(bytes, token);
  }

  static Future<String> saveEditableProjectFile({
    required String root,
    required String path,
    required Uint8List bytes,
    required String expectedToken,
  }) async {
    if (bytes.length > maxEditableBytes) {
      throw StateError('Edited files are limited to 2 MiB.');
    }
    final canonicalRoot = await Directory(root).resolveSymbolicLinks();
    final canonicalFile = await File(path).resolveSymbolicLinks();
    if (!canonicalFile.startsWith('$canonicalRoot${Platform.pathSeparator}')) {
      throw const FileSystemException('This path leaves the selected project.');
    }
    final relative = canonicalFile.substring(canonicalRoot.length + 1);
    final token = await desktopWorkspaceEditorChannel
        .invokeMethod<String>('save', {
          'root': canonicalRoot,
          'path': relative,
          'expected_token': expectedToken,
          'bytes': bytes,
        });
    if (token == null || token.isEmpty) {
      throw const FormatException(
        'The project editor returned no file version.',
      );
    }
    return token;
  }

  static Future<List<ChatFile>> pickAttachments(String roomId) async {
    final picked =
        await desktopAttachmentsChannel.invokeListMethod<Object?>('pick', {
          'room': roomId,
        }) ??
        const [];
    if (picked.length > maxFiles) {
      throw StateError('Choose up to five files at a time.');
    }
    final root = await roomAttachmentDirectory(roomId);
    await root.create(recursive: true);
    final files = <ChatFile>[];
    try {
      for (final raw in picked) {
        if (raw is! Map) throw const FormatException('Invalid selected file.');
        final selection = raw.cast<String, Object?>();
        final sourcePath = selection['path'];
        if (sourcePath is! String || sourcePath.isEmpty) {
          throw const FormatException('Selected item is not a local file.');
        }
        files.add(
          await stageLocalFile(
            roomId: roomId,
            root: root,
            sourcePath: sourcePath,
            name: selection['name'] as String? ?? 'attachment',
            mimeType:
                selection['mime_type'] as String? ?? 'application/octet-stream',
          ),
        );
      }
    } catch (_) {
      for (final file in files) {
        final staged = Directory('${root.path}/${file.id}');
        if (await staged.exists()) await staged.delete(recursive: true);
      }
      rethrow;
    }
    return files;
  }

  static Future<ChatFile> stageLocalFile({
    required String roomId,
    required Directory root,
    required String sourcePath,
    required String name,
    required String mimeType,
  }) async {
    if (await FileSystemEntity.type(sourcePath, followLinks: false) !=
        FileSystemEntityType.file) {
      throw const FormatException('Selected item is not a regular file.');
    }
    final source = File(sourcePath);
    final stat = await source.stat();
    if (stat.size > maxBytes) {
      throw StateError(
        '${_safeName(name)} exceeds the 20 MiB attachment limit.',
      );
    }
    final canonicalRoot = await root.resolveSymbolicLinks();
    final id = _newAttachmentId();
    final folder = Directory('$canonicalRoot/$id');
    await folder.create();
    try {
      final original = File('${folder.path}/original');
      final temporary = File('${folder.path}/original.tmp');
      final sink = temporary.openWrite();
      var bytes = 0;
      try {
        await for (final chunk in source.openRead()) {
          bytes += chunk.length;
          if (bytes > maxBytes) {
            throw StateError(
              '${_safeName(name)} exceeds the 20 MiB attachment limit.',
            );
          }
          sink.add(chunk);
        }
        await sink.flush();
      } finally {
        await sink.close();
      }
      await temporary.rename(original.path);
      final file = ChatFile(
        id: id,
        name: _safeName(name),
        mimeType: mimeType,
        bytes: bytes,
        note: bytes == 0 ? 'Empty file; original retained.' : '',
      );
      return prepareAttachment(roomId, file, root: Directory(canonicalRoot));
    } catch (_) {
      if (await folder.exists()) await folder.delete(recursive: true);
      rethrow;
    }
  }

  static Future<ChatFile> prepareAttachment(
    String roomId,
    ChatFile file, {
    Directory? root,
  }) async {
    if (!RegExp(r'^att_[a-f0-9]{32}$').hasMatch(file.id)) {
      throw const FormatException('Invalid attachment id.');
    }
    final attachmentRoot = root ?? await roomAttachmentDirectory(roomId);
    final canonicalRoot = await attachmentRoot.resolveSymbolicLinks();
    final folder = Directory('$canonicalRoot/${file.id}');
    final canonicalFolder = await folder.resolveSymbolicLinks();
    if (!canonicalFolder.startsWith('$canonicalRoot/')) {
      throw const FormatException('Attachment leaves this room.');
    }
    final original = File('$canonicalFolder/original');
    if (await FileSystemEntity.type(original.path, followLinks: false) !=
        FileSystemEntityType.file) {
      throw const FormatException('Attachment original is not a regular file.');
    }

    var hasText = false;
    var images = 0;
    var note = file.note;
    if (file.bytes > 0 && file.mimeType.startsWith('image/')) {
      try {
        final bytes = await original.readAsBytes();
        final buffer = await ui.ImmutableBuffer.fromUint8List(bytes);
        final descriptor = await ui.ImageDescriptor.encoded(buffer);
        try {
          if (descriptor.width <= 0 ||
              descriptor.height <= 0 ||
              descriptor.width * descriptor.height > 40000000) {
            throw StateError('Image dimensions exceed the preview limit.');
          }
          final codec = await descriptor.instantiateCodec(
            targetWidth: 1024,
            targetHeight: 1024,
          );
          try {
            final frame = await codec.getNextFrame();
            try {
              final preview = await frame.image.toByteData(
                format: ui.ImageByteFormat.png,
              );
              if (preview == null || preview.lengthInBytes > maxPreviewBytes) {
                throw StateError('Image preview exceeds the 2 MiB limit.');
              }
              await File('$canonicalFolder/image-0.jpg').writeAsBytes(
                preview.buffer.asUint8List(
                  preview.offsetInBytes,
                  preview.lengthInBytes,
                ),
                flush: true,
              );
              images = 1;
              note = 'Image preview ready; original retained.';
            } finally {
              frame.image.dispose();
            }
          } finally {
            codec.dispose();
          }
        } finally {
          descriptor.dispose();
          buffer.dispose();
        }
      } catch (_) {
        note = 'Original image retained; this format has no internal preview.';
      }
    } else if (file.bytes > 0 && file.mimeType.startsWith('video/')) {
      final analysis = await _extractVideoFrames(
        original,
        canonicalFolder,
        file.name,
      );
      images = analysis.images;
      note = analysis.note;
      if (analysis.context.isNotEmpty) {
        await File(
          '$canonicalFolder/context.txt',
        ).writeAsString(analysis.context, flush: true);
        hasText = true;
      }
    } else if (file.bytes > 0 && file.bytes <= maxTextBytes) {
      try {
        final bytes = await original.readAsBytes();
        final text = utf8.decode(bytes, allowMalformed: false);
        if (text.contains('\u0000')) throw const FormatException();
        final clipped = text.length <= 120000
            ? text
            : text.substring(0, 120000);
        await File(
          '$canonicalFolder/context.txt',
        ).writeAsString(clipped, flush: true);
        hasText = clipped.isNotEmpty;
        if (text.length > clipped.length) {
          note =
              'Text preview limited to 120,000 characters; original retained.';
        } else if (file.mimeType.startsWith('video/')) {
          note = 'Video retained; use Open With to play it.';
        } else if (file.mimeType == 'application/pdf' ||
            file.name.toLowerCase().endsWith('.pdf')) {
          note = 'PDF retained; use Open With to view it.';
        } else if (note.isEmpty) {
          note = 'Text preview ready; original retained.';
        }
      } on FormatException {
        note = _externalNote(file);
      }
    } else {
      note = _externalNote(file);
    }

    final prepared = ChatFile(
      id: file.id,
      name: file.name,
      mimeType: file.mimeType,
      bytes: file.bytes,
      hasText: hasText,
      images: images,
      note: note,
    );
    await _writeManifest(canonicalFolder, prepared);
    return prepared;
  }

  static Future<void> openAttachment(String roomId, String id) =>
      desktopAttachmentsChannel.invokeMethod<void>('open', {
        'room': roomId,
        'id': id,
      });

  static Future<void> openWith(String path) async {
    final type = await FileSystemEntity.type(path, followLinks: false);
    if (type != FileSystemEntityType.file) {
      throw const FileSystemException('Only regular files can be opened.');
    }
    final canonical = await File(path).resolveSymbolicLinks();
    try {
      await Process.start('gio', ['open', canonical]);
    } on ProcessException {
      await Process.start('xdg-open', [canonical]);
    }
  }

  static Future<String?> makeVideoThumbnailFromBytes(Uint8List bytes) async {
    if (bytes.isEmpty || bytes.length > maxBytes) return null;
    final temp = await Directory.systemTemp.createTemp('orgbot-video-');
    final input = File('${temp.path}/original');
    final output = '${temp.path}/frame.png';
    try {
      await input.writeAsBytes(bytes, flush: true);
      final process = await Process.start('ffmpeg', [
        '-hide_banner',
        '-loglevel',
        'error',
        '-nostdin',
        '-protocol_whitelist',
        'file,pipe',
        '-i',
        input.path,
        '-frames:v',
        '1',
        '-vf',
        'scale=640:360:force_original_aspect_ratio=decrease',
        '-f',
        'image2',
        '-fs',
        '2097152',
        '-y',
        output,
      ]);
      unawaited(process.stdout.drain<void>());
      unawaited(process.stderr.drain<void>());
      final code = await process.exitCode.timeout(
        const Duration(seconds: 8),
        onTimeout: () {
          process.kill(ProcessSignal.sigkill);
          return -1;
        },
      );
      if (code == -1) {
        await process.exitCode.timeout(
          const Duration(seconds: 2),
          onTimeout: () => -1,
        );
      }
      if (code != 0 || !await File(output).exists()) return null;
      return output;
    } on ProcessException {
      return null;
    } finally {
      if (!await File(output).exists() && await temp.exists()) {
        await temp.delete(recursive: true);
      }
    }
  }

  static const _videoDemuxers = <String, String>{
    '3g2': 'mov',
    '3gp': 'mov',
    'avi': 'avi',
    'flv': 'flv',
    'm2ts': 'mpegts',
    'm4v': 'mov',
    'mkv': 'matroska',
    'mov': 'mov',
    'mp4': 'mov',
    'mpeg': 'mpeg',
    'mpg': 'mpeg',
    'mts': 'mpegts',
    'ogv': 'ogg',
    'ts': 'mpegts',
    'webm': 'matroska',
    'wmv': 'asf',
  };

  static Future<_VideoAnalysis> _extractVideoFrames(
    File original,
    String attachmentFolder,
    String name,
  ) async {
    final extension = name.contains('.')
        ? name.split('.').last.toLowerCase()
        : '';
    final demuxer = _videoDemuxers[extension];
    if (demuxer == null) {
      return const _VideoAnalysis(
        images: 0,
        note: 'Original video retained; this format cannot be sampled locally.',
        context: '',
      );
    }
    final probe = await _runBoundedMediaProcess('ffprobe', [
      '-v',
      'error',
      '-protocol_whitelist',
      'file,pipe',
      '-f',
      demuxer,
      '-i',
      original.path,
      '-show_entries',
      'format=duration',
      '-of',
      'default=noprint_wrappers=1:nokey=1',
    ], timeout: const Duration(seconds: 6));
    final duration = double.tryParse(probe?.trim() ?? '');
    if (duration == null || !duration.isFinite || duration <= 0) {
      return const _VideoAnalysis(
        images: 0,
        note: 'Original video retained; duration could not be read safely.',
        context: '',
      );
    }

    final sampleCount = duration < 1.5
        ? 1
        : duration < 4
        ? 2
        : 4;
    final temp = await Directory.systemTemp.createTemp('orgbot-video-frames-');
    final timestamps = <double>[];
    var saved = 0;
    try {
      for (var index = 0; index < sampleCount; index++) {
        final seconds = sampleCount == 1
            ? 0.0
            : (duration * 0.95 * index / (sampleCount - 1));
        final output = '${temp.path}/frame-$index.jpg';
        final result = await _runBoundedMediaProcess('ffmpeg', [
          '-hide_banner',
          '-loglevel',
          'error',
          '-nostdin',
          '-protocol_whitelist',
          'file,pipe',
          '-threads',
          '1',
          '-filter_threads',
          '1',
          '-timelimit',
          '4',
          '-ss',
          seconds.toStringAsFixed(3),
          '-f',
          demuxer,
          '-i',
          original.path,
          '-map',
          '0:v:0',
          '-an',
          '-sn',
          '-dn',
          '-frames:v',
          '1',
          '-vf',
          'scale=640:360:force_original_aspect_ratio=decrease',
          '-f',
          'image2',
          '-fs',
          '$maxPreviewBytes',
          '-y',
          output,
        ], timeout: const Duration(seconds: 6));
        final frame = File(output);
        if (result == null || !await frame.exists()) continue;
        final size = await frame.length();
        if (size == 0 || size > maxPreviewBytes) continue;
        final temporary = File('$attachmentFolder/image-$saved.part');
        await frame.copy(temporary.path);
        await temporary.rename('$attachmentFolder/image-$saved.jpg');
        timestamps.add(seconds);
        saved++;
      }
    } on ProcessException {
      // Optional ffmpeg tools are not a prerequisite for retaining the original.
    } finally {
      if (await temp.exists()) await temp.delete(recursive: true);
    }
    if (saved == 0) {
      return const _VideoAnalysis(
        images: 0,
        note: 'Original video retained; no safe preview frames were available.',
        context: '',
      );
    }
    final labels = timestamps
        .map((time) => '${time.toStringAsFixed(2)}s')
        .join(', ');
    final sampled = 'Sampled $saved of $sampleCount video frames.';
    final context =
        'Video preview frames were sampled at $labels from a ${duration.toStringAsFixed(2)} second clip. '
        'These still images do not represent full motion or audio analysis; inspect the retained original for the complete video.\n';
    return _VideoAnalysis(
      images: saved,
      note:
          '$sampled Original retained; audio and full motion are not analyzed.',
      context: context,
    );
  }

  static Future<String?> _runBoundedMediaProcess(
    String executable,
    List<String> arguments, {
    required Duration timeout,
  }) async {
    Process? process;
    StreamSubscription<List<int>>? stdoutSubscription;
    StreamSubscription<List<int>>? stderrSubscription;
    final stdout = BytesBuilder(copy: false);
    final stdoutDone = Completer<void>();
    final stderrDone = Completer<void>();
    var tooMuchOutput = false;
    var timedOut = false;
    try {
      process = await Process.start(
        executable,
        arguments,
        environment: {
          'PATH': Platform.environment['PATH'] ?? '/usr/bin:/bin',
          'LANG': Platform.environment['LANG'] ?? 'C.UTF-8',
        },
        includeParentEnvironment: false,
        runInShell: false,
      );
      stdoutSubscription = process.stdout.listen(
        (chunk) {
          if (stdout.length + chunk.length > 8192) {
            tooMuchOutput = true;
            process!.kill(ProcessSignal.sigkill);
          } else {
            stdout.add(chunk);
          }
        },
        onDone: () {
          if (!stdoutDone.isCompleted) stdoutDone.complete();
        },
        onError: (Object _) {
          if (!stdoutDone.isCompleted) stdoutDone.complete();
        },
      );
      stderrSubscription = process.stderr.listen(
        (_) {},
        onDone: () {
          if (!stderrDone.isCompleted) stderrDone.complete();
        },
        onError: (Object _) {
          if (!stderrDone.isCompleted) stderrDone.complete();
        },
      );
      final exitCode =
          await (() async {
            final code = await process!.exitCode;
            await Future.wait([stdoutDone.future, stderrDone.future]);
            return code;
          })().timeout(
            timeout,
            onTimeout: () {
              timedOut = true;
              process!.kill(ProcessSignal.sigkill);
              return -1;
            },
          );
      if (timedOut || tooMuchOutput || exitCode != 0) return null;
      return utf8.decode(stdout.takeBytes(), allowMalformed: true);
    } on ProcessException {
      return null;
    } finally {
      if (process != null && (timedOut || tooMuchOutput)) {
        process.kill(ProcessSignal.sigkill);
        try {
          await process.exitCode.timeout(const Duration(seconds: 2));
        } on TimeoutException {
          // The process may already be reaped after SIGKILL.
        }
      }
      await stdoutSubscription?.cancel();
      await stderrSubscription?.cancel();
    }
  }

  static String? textPreview(Uint8List bytes) {
    if (bytes.length > maxTextBytes || bytes.contains(0)) return null;
    try {
      final text = utf8.decode(bytes, allowMalformed: false);
      return text.length > 120000 ? text.substring(0, 120000) : text;
    } on FormatException {
      return null;
    }
  }

  static Future<String> boundedGitOutput(
    String root,
    List<String> arguments, {
    int maxBytes = 256 * 1024,
  }) async {
    final canonicalRoot = await Directory(root).resolveSymbolicLinks();
    final environment = <String, String>{
      'PATH': Platform.environment['PATH'] ?? '/usr/bin:/bin',
      'HOME': Platform.environment['HOME'] ?? '/nonexistent',
      'LANG': Platform.environment['LANG'] ?? 'C.UTF-8',
      'GIT_OPTIONAL_LOCKS': '0',
      'GIT_CONFIG_NOSYSTEM': '1',
      'GIT_CONFIG_GLOBAL': '/dev/null',
      'GIT_PAGER': 'cat',
      'PAGER': 'cat',
      'GIT_TERMINAL_PROMPT': '0',
    };
    final process = await Process.start(
      'git',
      [
        '--no-pager',
        '-c',
        'core.fsmonitor=false',
        '-c',
        'core.hooksPath=/dev/null',
        '-c',
        'diff.external=',
        '-c',
        'core.pager=cat',
        ...arguments,
      ],
      workingDirectory: canonicalRoot,
      environment: environment,
      includeParentEnvironment: false,
      runInShell: false,
    );
    final out = BytesBuilder(copy: false);
    var tooLarge = false;
    var timedOut = false;
    final stdoutDone = Completer<void>();
    final stderrDone = Completer<void>();
    void collect(List<int> chunk) {
      if (tooLarge) return;
      if (out.length + chunk.length > maxBytes) {
        tooLarge = true;
        process.kill(ProcessSignal.sigkill);
      } else {
        out.add(chunk);
      }
    }

    final stdoutSub = process.stdout.listen(
      collect,
      onDone: stdoutDone.complete,
    );
    final stderrSub = process.stderr.listen(
      collect,
      onDone: stderrDone.complete,
    );
    var code = -1;
    try {
      code =
          await (() async {
            final result = await process.exitCode;
            await Future.wait([stdoutDone.future, stderrDone.future]);
            return result;
          })().timeout(
            const Duration(seconds: 10),
            onTimeout: () {
              timedOut = true;
              process.kill(ProcessSignal.sigkill);
              return -1;
            },
          );
    } finally {
      if (timedOut || tooLarge) {
        process.kill(ProcessSignal.sigkill);
        try {
          await process.exitCode.timeout(const Duration(seconds: 2));
        } on TimeoutException {
          // The killed process may already have been reaped by the runtime.
        }
        await stdoutSub.cancel();
        await stderrSub.cancel();
      }
    }
    if (timedOut) throw StateError('Git inspection timed out.');
    if (tooLarge) throw StateError('Git output exceeds the 256 KiB limit.');
    final text = utf8.decode(out.takeBytes(), allowMalformed: true);
    if (code != 0) {
      throw StateError(
        text.trim().isEmpty ? 'Git command failed.' : text.trim(),
      );
    }
    return text;
  }

  static String _externalNote(ChatFile file) {
    if (file.mimeType.startsWith('video/')) {
      return 'Video retained; use Open With to play it.';
    }
    if (file.mimeType == 'application/pdf' ||
        file.name.toLowerCase().endsWith('.pdf')) {
      return 'PDF retained; use Open With to view it.';
    }
    return 'Original retained; use Open With for this format.';
  }

  static Future<void> _writeManifest(String folder, ChatFile file) async {
    final manifest = File('$folder/manifest.json');
    final temporary = File('$folder/manifest.tmp');
    await temporary.writeAsString(jsonEncode(file.toJson()), flush: true);
    await temporary.rename(manifest.path);
  }

  static String _newAttachmentId() {
    final random = Random.secure();
    return 'att_${List<int>.generate(16, (_) => random.nextInt(256)).map((b) => b.toRadixString(16).padLeft(2, '0')).join()}';
  }

  static String _safeName(String value) {
    final cleaned = value.replaceAll(RegExp(r'[\x00-\x1f\\/]'), '_').trim();
    final normalized = cleaned.isEmpty ? 'attachment' : cleaned;
    return normalized.substring(0, normalized.length.clamp(0, 120));
  }
}

class EditableProjectFile {
  const EditableProjectFile(this.bytes, this.versionToken);

  final Uint8List bytes;
  final String versionToken;
}

class _VideoAnalysis {
  const _VideoAnalysis({
    required this.images,
    required this.note,
    required this.context,
  });

  final int images;
  final String note;
  final String context;
}
