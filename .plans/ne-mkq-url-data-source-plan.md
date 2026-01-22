# Implementation Plan: URL Data Source for Large-Scale Tests (ne-mkq)

*Plan created by planning task ne-48c*

## Summary

Enable large-scale tests to fetch data directly from URLs (like TU Berlin's tubcloud) without requiring pre-downloaded files.

## Recommended Approach: Buffered Download with URLDataSource

Create a new `URLDataSource` that downloads URL content to a temporary file, then reuses existing `FileSource` infrastructure.

**Why this approach:**
- Maximizes code reuse with existing FileSource
- Enables caching of downloaded files
- Simplifies error handling (download failures are separate from processing)
- Works seamlessly with existing systest infrastructure

## Library Recommendation: cpp-httplib

Add `cpp-httplib` to vcpkg.json:
- Header-only library (no linking complexity)
- Supports HTTPS via OpenSSL
- Simple, clean API
- Good timeout and error handling
- Active maintenance

**Alternative:** libcurl (if more advanced features needed later)

## Existing Code to Leverage

| Component | Location | Use |
|-----------|----------|-----|
| `NES::URI` | `nes-common/include/Util/URI.hpp` | URL parsing with boost::url |
| `FileSource` | `nes-sources/private/FileSource.hpp` | File reading after download |
| `Source` base | `nes-sources/include/Sources/Source.hpp` | Source interface pattern |
| `SourceDataProvider` | `nes-sources/include/Sources/SourceDataProvider.hpp` | Data provider pattern |
| `FileDataRegistry` | `nes-sources/registry/include/FileDataRegistry.hpp` | Registry pattern |
| `TCPDataServer` | `nes-plugins/Sources/TCPSource/TCPDataServer.hpp` | boost::asio networking example |

## Implementation Steps

### Step 1: Add cpp-httplib Dependency

**File:** `vcpkg/vcpkg.json`

```json
{
  "dependencies": [
    // ... existing deps ...
    "cpp-httplib"
  ]
}
```

### Step 2: Create URLDownloader Utility

**New File:** `nes-common/include/Util/URLDownloader.hpp`

```cpp
#pragma once
#include <filesystem>
#include <string>
#include <optional>
#include <Util/URI.hpp>

namespace NES {

struct DownloadResult {
    std::filesystem::path localPath;
    size_t bytesDownloaded;
    std::optional<std::string> contentType;
};

class URLDownloader {
public:
    /// Download URL content to target path
    /// @param url The URL to download from
    /// @param targetPath Local path to save the file
    /// @param timeoutSeconds Connection/read timeout
    /// @return DownloadResult on success
    /// @throws std::runtime_error on failure
    static DownloadResult downloadToFile(
        const URI& url,
        const std::filesystem::path& targetPath,
        int timeoutSeconds = 60);

    /// Check if a URL is reachable (HEAD request)
    static bool isReachable(const URI& url, int timeoutSeconds = 5);
};

} // namespace NES
```

**New File:** `nes-common/src/URLDownloader.cpp`

```cpp
#include <Util/URLDownloader.hpp>
#include <httplib.h>
#include <fstream>
#include <ErrorHandling.hpp>

namespace NES {

DownloadResult URLDownloader::downloadToFile(
    const URI& url,
    const std::filesystem::path& targetPath,
    int timeoutSeconds)
{
    auto boostUrl = static_cast<boost::urls::url>(url);

    httplib::SSLClient client(
        std::string(boostUrl.host()),
        boostUrl.port_number() ? boostUrl.port_number() : 443);
    client.set_connection_timeout(timeoutSeconds);
    client.set_read_timeout(timeoutSeconds);

    std::string path = std::string(boostUrl.path());
    if (!boostUrl.query().empty()) {
        path += "?" + std::string(boostUrl.query());
    }

    auto res = client.Get(path.c_str());
    if (!res) {
        throw std::runtime_error(fmt::format(
            "Failed to download from URL: {}", url.toString()));
    }

    if (res->status != 200) {
        throw std::runtime_error(fmt::format(
            "HTTP error {}: {}", res->status, url.toString()));
    }

    std::ofstream ofs(targetPath, std::ios::binary);
    ofs.write(res->body.data(), res->body.size());

    return DownloadResult{
        .localPath = targetPath,
        .bytesDownloaded = res->body.size(),
        .contentType = res->get_header_value("Content-Type")
    };
}

} // namespace NES
```

### Step 3: Create URLDataRegistry

**New File:** `nes-sources/registry/include/URLDataRegistry.hpp`

```cpp
#pragma once
#include <filesystem>
#include <string>
#include <Sources/SourceDataProvider.hpp>
#include <Util/Registry.hpp>

namespace NES {

using URLDataRegistryReturnType = PhysicalSourceConfig;

struct URLDataRegistryArguments {
    PhysicalSourceConfig physicalSourceConfig;
    std::string url;
    std::filesystem::path cacheDir;
};

class URLDataRegistry : public BaseRegistry<
    URLDataRegistry,
    std::string,
    URLDataRegistryReturnType,
    URLDataRegistryArguments>
{
};

} // namespace NES
```

### Step 4: Extend FileSource to Support URLs

**File:** `nes-sources/src/FileSource.cpp`

Add URL handling in `RegisterFileFileData`:

```cpp
FileDataRegistryReturnType FileDataGeneratedRegistrar::RegisterFileFileData(
    FileDataRegistryArguments args)
{
    // Check if file_path is actually a URL
    auto filePath = args.physicalSourceConfig.sourceConfig[
        std::string(SYSTEST_FILE_PATH_PARAMETER)];

    if (filePath.starts_with("http://") || filePath.starts_with("https://")) {
        // Download URL to cache directory
        auto cacheFile = args.testFilePath.parent_path() /
            ("url_cache_" + std::to_string(std::hash<std::string>{}(filePath)));

        if (!std::filesystem::exists(cacheFile)) {
            URLDownloader::downloadToFile(URI(filePath), cacheFile);
        }

        args.physicalSourceConfig.sourceConfig[
            std::string(SYSTEST_FILE_PATH_PARAMETER)] = cacheFile.string();
    }

    // ... existing code ...
}
```

### Step 5: Add Configuration Parameter

**File:** `nes-sources/private/FileSource.hpp`

```cpp
struct ConfigParametersCSV {
    static inline const DescriptorConfig::ConfigParameter<std::string> FILEPATH{...};

    // Add URL parameter (alternative to file_path)
    static inline const DescriptorConfig::ConfigParameter<std::string> URL{
        "url",
        std::nullopt,
        [](const auto& config) { return DescriptorConfig::tryGet(URL, config); }};

    // ... rest unchanged ...
};
```

### Step 6: Add Integration Test

**New File:** `nes-sources/tests/URLSourceTests.cpp`

```cpp
TEST(URLSourceTest, DownloadFromTubCloud) {
    const std::string testUrl =
        "https://tubcloud.tu-berlin.de/s/28Tr2wTd73Ggeed/download"
        "?files=MD5_d7e6113eb67d79644947ac6bc32a80bf";

    auto tempDir = std::filesystem::temp_directory_path() / "nes_url_test";
    std::filesystem::create_directories(tempDir);

    auto result = URLDownloader::downloadToFile(
        URI(testUrl),
        tempDir / "test_data.bin");

    EXPECT_TRUE(std::filesystem::exists(result.localPath));
    EXPECT_GT(result.bytesDownloaded, 0);

    // Cleanup
    std::filesystem::remove_all(tempDir);
}
```

## Files to Create/Modify Summary

| File | Action |
|------|--------|
| `vcpkg/vcpkg.json` | Add cpp-httplib dependency |
| `nes-common/CMakeLists.txt` | Add cpp-httplib to target_link_libraries |
| `nes-common/include/Util/URLDownloader.hpp` | **CREATE** - Download utility |
| `nes-common/src/URLDownloader.cpp` | **CREATE** - Download implementation |
| `nes-sources/registry/include/URLDataRegistry.hpp` | **CREATE** - Registry for URL sources |
| `nes-sources/private/FileSource.hpp` | Add URL config parameter |
| `nes-sources/src/FileSource.cpp` | Handle URL downloads in registry |
| `nes-sources/tests/URLSourceTests.cpp` | **CREATE** - Integration tests |

## Test URL

```
https://tubcloud.tu-berlin.de/s/28Tr2wTd73Ggeed/download?files=MD5_d7e6113eb67d79644947ac6bc32a80bf
```

Note: The MD5 in the filename can be used to validate download integrity.

## Streaming Alternative (Future Enhancement)

If streaming is needed later (for very large files), a `URLStreamSource` could be implemented:
- Use chunked transfer encoding
- Buffer HTTP response chunks into TupleBuffers directly
- Requires more complex state management
- Consider boost::beast for this use case

## Risk Mitigation

1. **Network failures**: Implement retry logic with exponential backoff
2. **Large files**: Add progress callback for long downloads
3. **HTTPS certificates**: Use system CA bundle, add option to skip verification for testing
4. **Timeouts**: Configurable connection and read timeouts
