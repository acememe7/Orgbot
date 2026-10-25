import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/protocol/models.dart';
import 'package:orgbot_app/workspace/desktop_files.dart';

Future<void> main() async {
  TestWidgetsFlutterBinding.ensureInitialized();
  var ffmpegAvailable = false;
  var ffprobeAvailable = false;
  try {
    final probe = await Process.run('ffmpeg', ['-version'], runInShell: false);
    ffmpegAvailable = probe.exitCode == 0;
  } on ProcessException {
    // The optional thumbnail test is skipped when FFmpeg is not installed.
  }
  try {
    final probe = await Process.run('ffprobe', ['-version'], runInShell: false);
    ffprobeAvailable = probe.exitCode == 0;
  } on ProcessException {
    // Video analysis is optional when FFprobe is not installed.
  }

  test('text previews reject binary and oversized content', () {
    expect(
      DesktopFiles.textPreview(Uint8List.fromList(utf8.encode('hello'))),
      'hello',
    );
    expect(DesktopFiles.textPreview(Uint8List.fromList([65, 0, 66])), isNull);
    expect(DesktopFiles.textPreview(Uint8List.fromList([0xff, 0xfe])), isNull);
    expect(
      DesktopFiles.textPreview(
        Uint8List.fromList(List<int>.filled(DesktopFiles.maxTextBytes + 1, 65)),
      ),
      isNull,
    );
  });

  test(
    'staging preserves binary originals and writes bounded text manifest',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'orgbot-attachment-test-',
      );
      final source = File('${root.path}/notes.txt');
      await source.writeAsString('room notes');
      final files = Directory('${root.path}/room/attachments');
      await files.create(recursive: true);
      try {
        final file = await DesktopFiles.stageLocalFile(
          roomId: 'room_test',
          root: files,
          sourcePath: source.path,
          name: 'notes.txt',
          mimeType: 'text/plain',
        );
        expect(file.hasText, isTrue);
        expect(file.note, contains('Text preview ready'));
        final folder = Directory('${files.path}/${file.id}');
        expect(
          await File('${folder.path}/original').readAsString(),
          'room notes',
        );
        expect(
          await File('${folder.path}/context.txt').readAsString(),
          'room notes',
        );
        final manifest = ChatFile.fromJson(
          jsonDecode(await File('${folder.path}/manifest.json').readAsString())
              as Map<String, Object?>,
        );
        expect(manifest.toJson(), file.toJson());
      } finally {
        await root.delete(recursive: true);
      }
    },
  );

  test(
    'attachment picker staging rejects symlinks and over-limit files',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'orgbot-attachment-limit-',
      );
      final source = File('${root.path}/large.bin');
      final handle = await source.open(mode: FileMode.write);
      await handle.setPosition(DesktopFiles.maxBytes + 1);
      await handle.writeByte(0);
      await handle.close();
      final links = Directory('${root.path}/room/attachments')
        ..createSync(recursive: true);
      final link = '${root.path}/link.bin';
      await Link(link).create(source.path);
      try {
        await expectLater(
          DesktopFiles.stageLocalFile(
            roomId: 'room_test',
            root: links,
            sourcePath: source.path,
            name: 'large.bin',
            mimeType: 'application/octet-stream',
          ),
          throwsStateError,
        );
        await expectLater(
          DesktopFiles.stageLocalFile(
            roomId: 'room_test',
            root: links,
            sourcePath: link,
            name: 'link.bin',
            mimeType: 'application/octet-stream',
          ),
          throwsFormatException,
        );
        expect(await links.list().length, 0);
      } finally {
        await root.delete(recursive: true);
      }
    },
  );

  test(
    'video thumbnail uses the first frame of a clip shorter than one second',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'orgbot-video-thumbnail-',
      );
      final video = File('${root.path}/short.mp4');
      String? thumbnail;
      try {
        final encoded = await Process.run('ffmpeg', [
          '-hide_banner',
          '-loglevel',
          'error',
          '-nostdin',
          '-f',
          'lavfi',
          '-i',
          'color=c=red:s=64x64:r=2:d=0.5',
          '-an',
          '-c:v',
          'mpeg4',
          '-y',
          video.path,
        ], runInShell: false);
        expect(encoded.exitCode, 0, reason: '${encoded.stderr}');
        thumbnail = await DesktopFiles.makeVideoThumbnailFromBytes(
          await video.readAsBytes(),
        );
        expect(thumbnail, isNotNull);
        expect(await File(thumbnail!).length(), greaterThan(8));
      } finally {
        final thumbFile = thumbnail == null ? null : File(thumbnail);
        if (thumbFile != null && await thumbFile.exists()) {
          await thumbFile.parent.delete(recursive: true);
        }
        await root.delete(recursive: true);
      }
    },
    skip: !ffmpegAvailable,
  );

  test(
    'video attachments store a timestamped bounded frame sample and original',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'orgbot-video-analysis-',
      );
      final video = File('${root.path}/short.mp4');
      final attachmentRoot = Directory('${root.path}/room/attachments')
        ..createSync(recursive: true);
      try {
        final encoded = await Process.run('ffmpeg', [
          '-hide_banner',
          '-loglevel',
          'error',
          '-nostdin',
          '-f',
          'lavfi',
          '-i',
          'color=c=blue:s=64x64:r=2:d=0.5',
          '-an',
          '-c:v',
          'mpeg4',
          '-y',
          video.path,
        ], runInShell: false);
        expect(encoded.exitCode, 0, reason: '${encoded.stderr}');
        final file = await DesktopFiles.stageLocalFile(
          roomId: 'room_test',
          root: attachmentRoot,
          sourcePath: video.path,
          name: 'short.mp4',
          mimeType: 'video/mp4',
        );
        expect(file.images, 1);
        expect(file.hasText, isTrue);
        expect(file.note, contains('audio and full motion are not analyzed'));
        final folder = Directory('${attachmentRoot.path}/${file.id}');
        expect(
          await File('${folder.path}/original').length(),
          await video.length(),
        );
        expect(
          await File('${folder.path}/image-0.jpg').length(),
          greaterThan(8),
        );
        final context = await File('${folder.path}/context.txt').readAsString();
        expect(context, contains('0.00s'));
        expect(context, contains('not represent full motion'));
        final manifest = ChatFile.fromJson(
          jsonDecode(await File('${folder.path}/manifest.json').readAsString())
              as Map<String, Object?>,
        );
        expect(manifest.images, 1);
        expect(manifest.note, contains('Original retained'));
      } finally {
        await root.delete(recursive: true);
      }
    },
    skip: !ffmpegAvailable || !ffprobeAvailable,
  );

  test(
    'Git inspection uses the selected directory, not inherited Git state',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'orgbot-git-readonly-',
      );
      try {
        final init = await Process.run('git', [
          'init',
          '--quiet',
          root.path,
        ], runInShell: false);
        expect(init.exitCode, 0);
        final result = await DesktopFiles.boundedGitOutput(root.path, [
          '--no-optional-locks',
          'status',
          '--short',
          '--branch',
        ]);
        expect(result, contains('## No commits yet on'));
      } finally {
        await root.delete(recursive: true);
      }
    },
  );
}
