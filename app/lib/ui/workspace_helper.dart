import 'dart:io';

Future<String?> locateWorkspaceHelper({
  String? override,
  String? executablePath,
}) async {
  if (override != null) return override;
  final binaryPath =
      executablePath ?? File(Platform.resolvedExecutable).absolute.path;
  const sourceBuildMarker = '/app/build/';
  final sourceBuildIndex = binaryPath.lastIndexOf(sourceBuildMarker);
  if (sourceBuildIndex >= 0) {
    final sourceRoot = binaryPath.substring(0, sourceBuildIndex);
    final sourceHelper = '$sourceRoot/app/packaging/orgbot-workspace.py';
    if (await File(sourceHelper).exists()) return sourceHelper;
  }
  const installed = '/usr/lib/orgbot/orgbot-workspace.py';
  if (await File(installed).exists()) return installed;
  return null;
}
