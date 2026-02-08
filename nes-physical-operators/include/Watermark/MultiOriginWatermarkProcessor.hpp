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
#include <cstdint>
#include <memory>
#include <string>
#include <vector>
#include <Identifiers/Identifiers.hpp>
#include <Sequencing/RangeCompletionTracker.hpp>
#include <Sequencing/SequenceData.hpp>
#include <Time/Timestamp.hpp>
#include <Util/Common.hpp>
#include <folly/Synchronized.h>

namespace NES
{

/// @brief A multi origin version of the watermark processor.
/// Each origin tracks range completion and associated watermark values independently.
/// The global watermark is the minimum completed watermark across all origins.
class MultiOriginWatermarkProcessor
{
public:
    explicit MultiOriginWatermarkProcessor(const std::vector<OriginId>& origins);
    static std::shared_ptr<MultiOriginWatermarkProcessor> create(const std::vector<OriginId>& origins);

    /// @brief Updates the watermark timestamp and origin and emits the current watermark.
    [[nodiscard]] Timestamp updateWatermark(Timestamp ts, SequenceData sequenceData, OriginId origin) const;

    /// @brief Returns the current watermark across all origins
    [[nodiscard]] Timestamp getCurrentWatermark() const;

    std::string getCurrentStatus();

private:
    const std::vector<OriginId> origins;
    mutable std::vector<folly::Synchronized<RangeCompletionTracker<uint64_t>>> trackers;
};

}
