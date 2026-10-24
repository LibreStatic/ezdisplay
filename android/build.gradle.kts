buildscript {
    dependencies {
        // AGP 9 has built-in Kotlin (2.2.x); pin a newer Kotlin Gradle plugin on the build classpath.
        classpath(libs.kotlin.gradle.plugin)
    }
}

plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.kotlin.compose) apply false
    alias(libs.plugins.roborazzi) apply false
}
