// ezdisplay Android client.
// - The native library (crates/ezd-android) is built by cargo-ndk into build/generated, never src/.
// - versionName comes from the Rust workspace version in the root Cargo.toml.
import javax.inject.Inject

plugins {
    alias(libs.plugins.android.application)
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
    compileSdk = 36
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "io.github.facum.ezdisplay"
        minSdk = 26
        targetSdk = 36
        versionName = workspaceVersion
        versionCode = workspaceVersionCode
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
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

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
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
                    include("crates/ezd-proto/**", "crates/ezd-link/**")
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
