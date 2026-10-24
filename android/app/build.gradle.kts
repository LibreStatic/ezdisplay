// ezdisplay Android client.
// - The native library (crates/ezd-android) is built by cargo-ndk into build/generated, never src/.
// - versionName comes from the Rust workspace version in the root Cargo.toml.
import javax.inject.Inject

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.roborazzi)
}

val repoRootDir: File = rootDir.resolve("..").canonicalFile

val workspaceVersion: String = run {
    val cargo = repoRootDir.resolve("Cargo.toml").readText()
    val section = cargo.substringAfter("[workspace.package]", "")
    Regex("""(?m)^version\s*=\s*"([^"]+)"""").find(section)?.groupValues?.get(1)
        ?: error("[workspace.package] version not found in ${repoRootDir.resolve("Cargo.toml")}")
}

/** 0.1.0 -> 1000; MAJOR*1_000_000 + MINOR*1_000 + PATCH. */
val workspaceVersionCode: Int = workspaceVersion.substringBefore('-').split('.')
    .map { it.toInt() }.let { (major, minor, patch) -> major * 1_000_000 + minor * 1_000 + patch }
    .coerceAtLeast(1)

android {
    namespace = "io.github.facum.ezdisplay"
    compileSdk = 37
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "io.github.facum.ezdisplay"
        minSdk = 26
        targetSdk = 36
        versionName = workspaceVersion
        versionCode = workspaceVersionCode
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }

    androidResources {
        generateLocaleConfig = true
    }

    flavorDimensions += "distribution"
    productFlavors {
        // GitHub builds: may self-update (REQUEST_INSTALL_PACKAGES lives in src/full).
        create("full") { dimension = "distribution" }
        // Play Store builds: policy-safe, no install-packages permission.
        create("play") { dimension = "distribution" }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    buildFeatures { compose = true }

    testOptions {
        unitTests {
            isIncludeAndroidResources = true
            // Robolectric reaches into JDK internals, which newer JDKs (25+) no longer open by default.
            all { it.jvmArgs("--add-exports=java.base/jdk.internal.access=ALL-UNNAMED") }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.foundation)
    implementation(libs.compose.material3)
    implementation(libs.androidx.activity.compose)

    testImplementation(platform(libs.compose.bom))
    testImplementation(libs.junit)
    testImplementation(libs.robolectric)
    testImplementation(libs.androidx.test.core)
    testImplementation(libs.compose.ui.test.junit4)
    testImplementation(libs.roborazzi)
    testImplementation(libs.roborazzi.compose)
    testImplementation(libs.roborazzi.junit.rule)
    debugImplementation(platform(libs.compose.bom))
    debugImplementation(libs.compose.ui.test.manifest)
}

/** Builds libezd_android.so for every ABI with cargo-ndk into [outputDir] (`<abi>/libezd_android.so`). */
abstract class CargoNdkTask @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    @get:Internal abstract val repoRoot: DirectoryProperty
    @get:InputFiles @get:PathSensitive(PathSensitivity.RELATIVE) abstract val rustSources: ConfigurableFileCollection
    @get:Input abstract val release: Property<Boolean>
    @get:Input abstract val abis: ListProperty<String>
    @get:Input abstract val ndkDir: Property<String>
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun build() {
        val out = outputDir.get().asFile
        out.deleteRecursively()
        out.mkdirs()
        val args = mutableListOf("cargo", "ndk", "--platform", "26")
        abis.get().forEach { args += listOf("-t", it) }
        args += listOf("-o", out.absolutePath, "build", "-p", "ezd-android")
        if (release.get()) args += "--release"
        exec.exec {
            workingDir = repoRoot.get().asFile
            commandLine(args)
            val cargoBin = File(System.getProperty("user.home"), ".cargo/bin").absolutePath
            environment("PATH", cargoBin + File.pathSeparator + (System.getenv("PATH") ?: ""))
            environment("ANDROID_NDK_HOME", ndkDir.get())
        }
    }
}

androidComponents {
    onVariants { variant ->
        val cap = variant.name.replaceFirstChar { it.uppercase() }
        val cargo = tasks.register<CargoNdkTask>("cargoNdk$cap") {
            group = "build"
            description = "Builds libezd_android.so with cargo-ndk for ${variant.name}"
            repoRoot.set(repoRootDir)
            rustSources.from(
                fileTree(repoRootDir) {
                    include("Cargo.toml", "Cargo.lock")
                    include("crates/ezd-android/**", "crates/ezd-client-core/**")
                    include("crates/ezd-proto/**", "crates/ezd-link/**", "crates/ezd-input/**")
                    exclude("**/target/**")
                },
            )
            release.set(variant.buildType == "release")
            abis.set(listOf("arm64-v8a", "x86_64"))
            ndkDir.set(androidComponents.sdkComponents.ndkDirectory.map { it.asFile.absolutePath })
        }
        variant.sources.jniLibs?.addGeneratedSourceDirectory(cargo, CargoNdkTask::outputDir)
    }
}
