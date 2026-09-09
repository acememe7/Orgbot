import java.util.Properties

plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

val qaBuild = providers.gradleProperty("orgbotQa").orNull == "true"
require(qaBuild || !providers.gradleProperty("target").orNull.orEmpty().contains("integration_test")) {
    "Integration tests must use -PorgbotQa=true to protect the normal app and its data."
}
val releaseKeys = Properties()
val releaseKeyFile = rootProject.file("key.properties")
if (releaseKeyFile.exists()) releaseKeyFile.inputStream().use { releaseKeys.load(it) }

android {
    namespace = "com.orgbot.app"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        // TODO: Specify your own unique Application ID (https://developer.android.com/studio/build/application-id.html).
        applicationId = if (qaBuild) "com.orgbot.app.qa" else "com.orgbot.app"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = 24
        ndk { abiFilters += "arm64-v8a" }
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    signingConfigs {
        if (releaseKeyFile.exists()) create("ownerRelease") {
            keyAlias = releaseKeys.getProperty("keyAlias")
            keyPassword = releaseKeys.getProperty("keyPassword")
            storeFile = file(releaseKeys.getProperty("storeFile"))
            storePassword = releaseKeys.getProperty("storePassword")
        }
    }
    // Flutter native-asset caches can retain libraries from earlier QA targets.
    // A single-ABI APK must not advertise architectures missing its engine.
    packaging {
        jniLibs.excludes += setOf("**/armeabi-v7a/**", "**/x86/**", "**/x86_64/**")
    }
    buildTypes {
        release {
            signingConfig = if (releaseKeyFile.exists()) signingConfigs.getByName("ownerRelease") else null
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}

// An APK must never silently bundle an old FFI ABI.
val buildRoomEngine by tasks.registering(Exec::class) {
    workingDir(rootProject.file("../.."))
    commandLine("bash", "core/scripts/build-android.sh")
}
tasks.named("preBuild") { dependsOn(buildRoomEngine) }
gradle.taskGraph.whenReady {
    if (allTasks.any { it.name == "assembleRelease" || it.name == "bundleRelease" } && !releaseKeyFile.exists()) {
        throw GradleException("Release signing requires android/key.properties. Use a debug build for development installs.")
    }
}
