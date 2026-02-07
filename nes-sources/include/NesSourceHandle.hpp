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

#pragma once

#include <atomic>
#include <cstdint>
#include <memory>
#include <optional>
#include <stop_token>
#include <string>
#include <Execution/NesSourceAdapter.hpp>
#include <Execution/NesStageContext.hpp>
#include <Identifiers/Identifiers.hpp>
#include <Sources/Source.hpp>

namespace NES
{

/// SourceHandle implementation that wraps NES Source instances for the adaptive engine.
///
/// This class adapts NES's push-based Source interface to the adaptive_engine's pull-based
/// SourceHandle interface. It wraps a NES::Source and converts its fillTupleBuffer method
/// into the next_buffer() pull model expected by the adaptive engine.
///
/// Threading model: The adaptive engine executor calls next_buffer() from its worker threads.
/// This adapter is synchronous - it blocks on fillTupleBuffer() until data is available.
class NesSourceHandle final : public NesSourceAdapter
{
public:
    /// Construct a NesSourceHandle wrapping a NES Source.
    /// @param source The underlying NES Source implementation to wrap
    /// @param originId Unique identifier for this source
    /// @param bufferProvider Buffer provider for allocating buffers
    NesSourceHandle(
        std::unique_ptr<Source> source,
        OriginId originId,
        std::shared_ptr<AbstractBufferProvider> bufferProvider);

    ~NesSourceHandle() override = default;

    // Non-copyable, non-movable
    NesSourceHandle(const NesSourceHandle&) = delete;
    NesSourceHandle& operator=(const NesSourceHandle&) = delete;
    NesSourceHandle(NesSourceHandle&&) = delete;
    NesSourceHandle& operator=(NesSourceHandle&&) = delete;

    /// Get the next buffer from the source.
    /// @param ctx Execution context for this invocation
    /// @return Buffer handle if data available, nullopt if source is exhausted (EoS)
    std::optional<TupleBuffer> nextBuffer(NesStageContext& ctx) override;

    /// Open the source for reading.
    /// @param ctx Execution context for this invocation
    void open(NesStageContext& ctx) override;

    /// Close the source.
    /// @param ctx Execution context for this invocation
    void close(NesStageContext& ctx) override;

    /// Get the unique identifier for this source.
    /// @return Source identifier string
    std::string getId() const override;

    /// Request the source to stop producing data.
    /// This signals the stop token used in fillTupleBuffer.
    void requestStop() override;

private:
    std::unique_ptr<Source> source_;
    OriginId originId_;
    std::shared_ptr<AbstractBufferProvider> bufferProvider_;
    std::stop_source stopSource_;
    std::atomic<uint64_t> sequenceNumber_{1};
    bool opened_{false};
};

}  // namespace NES
