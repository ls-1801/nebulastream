/*
    Licensed under the Apache License, Version 2.0 (the "License");
    you may not use this file except in compliance with the License.
    You may obtain a copy of the License at

        https://www.apache.org/licenses/LICENSE-2.0

    Unless required by applicable law or agreed to in writing, software
    distributed under the License is distributed on an "AS IS" BASIS,
    WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
    See the License for the specific language governing permissions and
    limitations under the License.
*/

#include <FileSource.hpp>

#include <algorithm>
#include <cerrno>
#include <cstdlib>
#include <cstring>
#include <format>
#include <fstream>
#include <ios>
#include <istream>
#include <memory>
#include <ostream>
#include <stop_token>
#include <string>
#include <unordered_map>
#include <utility>
#include <Configurations/Descriptor.hpp>
#include <Runtime/AbstractBufferProvider.hpp>
#include <Runtime/TupleBuffer.hpp>
#include <Sources/Source.hpp>
#include <Sources/SourceDescriptor.hpp>
#include <Util/Files.hpp>
#include <ErrorHandling.hpp>
#include <FileDataRegistry.hpp>
#include <InlineDataRegistry.hpp>
#include <SourceRegistry.hpp>
#include <SourceValidationRegistry.hpp>

namespace NES
{

FileSource::FileSource(const SourceDescriptor& sourceDescriptor)
    : filePath(sourceDescriptor.getFromConfig(ConfigParametersCSV::FILEPATH)),
      compressionType(sourceDescriptor.getFromConfig(ConfigParametersCSV::COMPRESSION))
{
}

void FileSource::open(std::shared_ptr<AbstractBufferProvider>)
{
    const auto realCSVPath = std::unique_ptr<char, decltype(std::free)*>{realpath(this->filePath.c_str(), nullptr), std::free};
    this->inputFile = std::ifstream(realCSVPath.get(), std::ios::binary);
    if (not this->inputFile)
    {
        throw InvalidConfigParameter("Could not determine absolute pathname: {} - {}", this->filePath.c_str(), getErrorMessageFromERRNO());
    }

    if (this->compressionType == "zstd")
    {
        this->zstdContext = ZSTD_createDCtx();
        if (this->zstdContext == nullptr)
        {
            throw InvalidConfigParameter("Failed to create ZSTD decompression context");
        }
        /// Allocate buffers for streaming decompression
        /// Use ZSTD recommended sizes for streaming
        this->compressedBuffer.resize(ZSTD_DStreamInSize());
        this->decompressedBuffer.resize(ZSTD_DStreamOutSize());
        this->decompressedOffset = 0;
        this->decompressedSize = 0;
        this->endOfCompressedStream = false;
    }
    else if (this->compressionType != "none" && !this->compressionType.empty())
    {
        throw InvalidConfigParameter("Unsupported compression type: {}", this->compressionType);
    }
}

void FileSource::close()
{
    this->inputFile.close();
    if (this->zstdContext != nullptr)
    {
        ZSTD_freeDCtx(this->zstdContext);
        this->zstdContext = nullptr;
    }
}

bool FileSource::decompressNextBlock()
{
    if (this->endOfCompressedStream)
    {
        return false;
    }

    /// Read compressed data from file
    this->inputFile.read(this->compressedBuffer.data(), static_cast<std::streamsize>(this->compressedBuffer.size()));
    const auto compressedBytesRead = static_cast<size_t>(this->inputFile.gcount());

    if (compressedBytesRead == 0)
    {
        this->endOfCompressedStream = true;
        return false;
    }

    /// Set up input buffer for decompression
    ZSTD_inBuffer input = {this->compressedBuffer.data(), compressedBytesRead, 0};

    /// Decompress until all input is consumed
    this->decompressedSize = 0;
    while (input.pos < input.size)
    {
        ZSTD_outBuffer output = {
            this->decompressedBuffer.data() + this->decompressedSize,
            this->decompressedBuffer.size() - this->decompressedSize,
            0};

        const size_t ret = ZSTD_decompressStream(this->zstdContext, &output, &input);
        if (ZSTD_isError(ret))
        {
            throw InvalidConfigParameter("ZSTD decompression error: {}", ZSTD_getErrorName(ret));
        }

        this->decompressedSize += output.pos;

        /// If output buffer is full, stop and process what we have
        if (this->decompressedSize >= this->decompressedBuffer.size())
        {
            break;
        }
    }

    this->decompressedOffset = 0;
    return this->decompressedSize > 0;
}

Source::FillTupleBufferResult FileSource::fillTupleBuffer(TupleBuffer& tupleBuffer, const std::stop_token&)
{
    if (this->compressionType == "zstd")
    {
        /// Handle zstd decompressed data
        size_t totalBytesWritten = 0;
        const size_t bufferSize = tupleBuffer.getBufferSize();
        auto* destBuffer = tupleBuffer.getAvailableMemoryArea<char>().data();

        while (totalBytesWritten < bufferSize)
        {
            /// If we have decompressed data available, copy it
            if (this->decompressedOffset < this->decompressedSize)
            {
                const size_t availableDecompressed = this->decompressedSize - this->decompressedOffset;
                const size_t spaceInBuffer = bufferSize - totalBytesWritten;
                const size_t bytesToCopy = std::min(availableDecompressed, spaceInBuffer);

                std::memcpy(destBuffer + totalBytesWritten, this->decompressedBuffer.data() + this->decompressedOffset, bytesToCopy);

                this->decompressedOffset += bytesToCopy;
                totalBytesWritten += bytesToCopy;
            }
            else
            {
                /// Need more decompressed data
                if (!this->decompressNextBlock())
                {
                    break; /// End of stream
                }
            }
        }

        this->totalNumBytesRead += totalBytesWritten;
        if (totalBytesWritten == 0)
        {
            return FillTupleBufferResult::eos();
        }
        return FillTupleBufferResult::withBytes(static_cast<int64_t>(totalBytesWritten));
    }

    /// Original uncompressed file reading
    this->inputFile.read(
        tupleBuffer.getAvailableMemoryArea<std::istream::char_type>().data(), static_cast<std::streamsize>(tupleBuffer.getBufferSize()));
    const auto numBytesRead = this->inputFile.gcount();
    this->totalNumBytesRead += numBytesRead;
    if (numBytesRead == 0)
    {
        return FillTupleBufferResult::eos();
    }
    return FillTupleBufferResult::withBytes(numBytesRead);
}

DescriptorConfig::Config FileSource::validateAndFormat(std::unordered_map<std::string, std::string> config)
{
    return DescriptorConfig::validateAndFormat<ConfigParametersCSV>(std::move(config), NAME);
}

std::ostream& FileSource::toString(std::ostream& str) const
{
    str << std::format(
        "\nFileSource(filepath: {}, compression: {}, totalNumBytesRead: {})", this->filePath, this->compressionType, this->totalNumBytesRead.load());
    return str;
}

SourceValidationRegistryReturnType RegisterFileSourceValidation(SourceValidationRegistryArguments sourceConfig)
{
    return FileSource::validateAndFormat(std::move(sourceConfig.config));
}

SourceRegistryReturnType SourceGeneratedRegistrar::RegisterFileSource(SourceRegistryArguments sourceRegistryArguments)
{
    return std::make_unique<FileSource>(sourceRegistryArguments.sourceDescriptor);
}

InlineDataRegistryReturnType InlineDataGeneratedRegistrar::RegisterFileInlineData(InlineDataRegistryArguments systestAdaptorArguments)
{
    if (systestAdaptorArguments.physicalSourceConfig.sourceConfig.contains(std::string(SYSTEST_FILE_PATH_PARAMETER)))
    {
        throw InvalidConfigParameter("Mock FileSource cannot use given inline data if a 'file_path' is set");
    }

    systestAdaptorArguments.physicalSourceConfig.sourceConfig.try_emplace(
        std::string(SYSTEST_FILE_PATH_PARAMETER), systestAdaptorArguments.testFilePath.string());


    if (std::ofstream testFile(systestAdaptorArguments.testFilePath); testFile.is_open())
    {
        /// Write inline tuples to test file.
        for (const auto& tuple : systestAdaptorArguments.tuples)
        {
            testFile << tuple << "\n";
        }
        testFile.flush();
        return systestAdaptorArguments.physicalSourceConfig;
    }
    throw TestException("Could not open source file \"{}\"", systestAdaptorArguments.testFilePath);
}

FileDataRegistryReturnType FileDataGeneratedRegistrar::RegisterFileFileData(FileDataRegistryArguments systestAdaptorArguments)
{
    if (systestAdaptorArguments.physicalSourceConfig.sourceConfig.contains(std::string(SYSTEST_FILE_PATH_PARAMETER)))
    {
        throw InvalidConfigParameter("The mock file data source cannot be used if the file_path parameter is already set.");
    }

    systestAdaptorArguments.physicalSourceConfig.sourceConfig.emplace(
        std::string(SYSTEST_FILE_PATH_PARAMETER), systestAdaptorArguments.testFilePath.string());

    return systestAdaptorArguments.physicalSourceConfig;
}


}
